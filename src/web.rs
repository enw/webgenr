use crate::document::Document;
use anyhow::Context;
use handlebars::Handlebars;
use rust_embed::RustEmbed;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};
use std::ffi::OsStr;

pub struct Web<'a> {
    pub in_path: PathBuf,
    pub out_path: PathBuf,
    pub template_dir_path: PathBuf,
    doc_list: Vec<Document>,
    pub template_registry: Handlebars<'a>,
}

#[derive(RustEmbed)]
#[folder = "templates/"]
#[exclude = ".*"]   // ignore hidden files
struct Asset;

// return true if the DirEntry represents a hidden file or directory
fn is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with("."))
        .unwrap_or(false)
}

fn new_doc_list<P: AsRef<Path>>(path_ref: P) -> anyhow::Result<Vec<Document>> {
    let mut vec: Vec<Document> = Vec::new();
    let root = path_ref.as_ref().to_path_buf();

    let walker = WalkDir::new(root).follow_links(true).into_iter();
    for entry_result in walker.filter_entry(|e| !is_hidden(e)) {
        let entry = entry_result?;
        let path = entry.path();
        if fs::metadata(path)?.is_file() {
            let doc = Document::new(path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            vec.push(doc);
        }
    }
    Ok(vec)
}

impl Web<'_> {
    fn create_all_parent_dir(path: &Path) -> std::io::Result<()> {
        let dir = path.parent().unwrap();
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    // copy embedded templates into given directory path
    fn inflate_default_templates<P: AsRef<Path>>(templatedir_path: P) -> anyhow::Result<()> {
        info!("inflating default templates");
        for relative_path_str in Asset::iter() {
            info!("  {}", relative_path_str);
            let relative_path = PathBuf::from(relative_path_str.to_string());
            let new_template_path = Path::new("").join(&templatedir_path).join(&relative_path);
            Self::create_all_parent_dir(&new_template_path)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .open(&new_template_path)
                .unwrap();
            file.write_all(&Asset::get(&relative_path_str).unwrap().data.as_ref())?;
        }
        Ok(())
    }

    // copy files recursively from `source_dir` to `dest_dir`
    // omitting files with extension `omit_ext` (.gitignore syntax from globwalk crate)
    fn copy_files<P: AsRef<Path>>(source_dir: P, dest_dir: P, omit_ext: &str) -> anyhow::Result<()> {
        info!("copyfiles, omitting {}", omit_ext);
        info!(" std::env::current_dir: {:?}",  std::env::current_dir());
    let walker = WalkDir::new(&source_dir).follow_links(true).into_iter();
    for entry_result in walker
        .filter_entry(|e| !is_hidden(e) && e.path().extension() != Some(OsStr::new(omit_ext))) {
            if let Ok(dir_entry) = entry_result  {
                let rel_path = dir_entry.path()
                    .strip_prefix(&source_dir)
                    .expect("strip prefix match");

                let dest_path = PathBuf::from(dest_dir.as_ref()).join(rel_path);
                if dir_entry.path().is_dir() {
                    info!("  dir: {:?}", dest_path);
                    fs::create_dir_all(&dest_path)?;
                } else {
                    // copy file
                    info!("  file: {:?}", dir_entry.path());

                    match fs::copy(dir_entry.path(), &dest_path) {
                        Ok(bytes) => info!(
                            "copy {} bytes-> {}\t{}",
                            bytes,
                            dir_entry.path().display(),
                            &dest_path.display()
                        ),
                        Err(e) => anyhow::bail!("error: {}, failed to copy from: {} to {}",
                            e,
                            dir_entry.path().display(),
                            &dest_path.display())
                    }
                }
            }
        }
        Ok(())
    }

    fn path_not_found<P: AsRef<Path>>(path: P) -> anyhow::Result<bool> {
       if let Err(err) = fs::metadata(&path) {
            match err.kind() {
                std::io::ErrorKind::NotFound => {
                    return Ok(true)
                }
                _ => {
                    error!("Error finding templates directory");
                    return Err(err.into())
                }
            }
       }
       Ok(false)   // path was found
    }

    // creates required folders (but does not delete any old files)
    pub fn new<P: AsRef<Path>>(in_path: P, out_path: P, templatedir_path: P) -> anyhow::Result<Self> {

       fs::create_dir_all(&in_path)?;
        // create templates directory and fill with default templates if needed
       if Self::path_not_found(&templatedir_path)? {
            fs::create_dir_all(&templatedir_path)?;
            Self::inflate_default_templates(&templatedir_path)?;
        }

        let mut handlebars = Handlebars::new();
        handlebars.register_templates_directory(".hbs", &templatedir_path)?;
        handlebars.register_escape_fn(handlebars::no_escape);
        Ok(Web {
            in_path: in_path.as_ref().to_path_buf(),
            out_path: out_path.as_ref().to_path_buf(),
            template_dir_path: templatedir_path.as_ref().to_path_buf(),
            doc_list: new_doc_list(in_path)?,
            template_registry: handlebars,
        })
    }

    // given a `source_path` return corresponding output path
    fn outpath(&self, doc: &Document) -> std::io::Result<PathBuf> {
       let rel_path = doc
            .source_path
            .strip_prefix(&self.in_path)
            .expect("strip prefix match");
        Ok(self.out_path.join(rel_path))
    }

    fn make_book_internal(&self, author: &str, title: &str) -> anyhow::Result<()> {
        use epub_builder::EpubBuilder;
        use epub_builder::EpubContent;
        use epub_builder::ReferenceType;
        use epub_builder::ZipLibrary;
        use std::fs::File;
        use anyhow::anyhow;

        let writer = std::fs::File::create("book.epub")?;
        let zip_lib = ZipLibrary::new().map_err(|err| anyhow!("initializing zip {:#?}", err))?;
        let mut epub = EpubBuilder::new(zip_lib)
            .map_err(|err| anyhow!("initializing epub {:#?}", err))?;

        epub.add_author(author);
        epub.set_title(title);
        let mut chapter_number = 1;

        for doc in &self.doc_list {
            let file_stem = doc.file_stem()?;

            match file_stem {
                "cover" | "_cover" =>  {
                    println!("cover: {}", doc.source_path.display());
                    let default_extension = "png";
                    let extension = match doc.source_path.file_stem() {
                        Some(os_str) => {
                            match os_str.to_str() {
                                Some(str) => str,
                                None => {
                                    println!("can't convert file extension {:?} to str", os_str);
                                    default_extension
                                },
                            }
                        },
                        None => {
                            println!("no file extension for cover image, assuming png");
                            default_extension
                        },
                    };
                    epub.add_cover_image(&doc.source_path,
                                File::open(&doc.source_path)?,
                                format!("image/{}", extension))
                                .map_err(|err| anyhow!("adding cover image {:#?}", err))?;

                },
                "title" | "_title" =>  {
                    println!("title page: {}", doc.source_path.display());
                    let file_name = doc.source_path.file_name().unwrap().to_string_lossy();
                    epub.add_content(
                        EpubContent::new(file_name, File::open(&doc.source_path)?)
                            .title("Title Page")
                            .reftype(ReferenceType::TitlePage),
                        )
                        .map_err(|err| anyhow!("adding title page to epub {:#?}", err))?;
                },
                _ => {
                    let default_zip_path = format!("chapter{}.xhtml", chapter_number);
                    let chapter_title = format!("Chapter {}", chapter_number);  // TODO: get from YAML front matter
                    let zip_path = match doc.source_path.file_stem() {
                        Some(os_str) => format!("{}.xhtml", os_str.to_string_lossy()),
                        None => default_zip_path,
                    };
                    println!("adding {}\tas {},\ttitle: {}", doc.source_path.display(), zip_path, chapter_title);
                    epub.add_content(
                        EpubContent::new(zip_path, File::open(&doc.source_path)?)
                            .title(chapter_title)
                            .reftype(ReferenceType::Text),
                    )
                    .map_err(|err| anyhow!("adding content to epub {:#?}", err))?;
                    chapter_number = chapter_number +1;

                }
            } // match file_stem
        }
        epub.generate(writer)
        .map_err(|err| anyhow!("generating epub {:#?}", err))?;

        Ok(())
    }

    // if folder exists, delete it & all contents and create new
    fn clean_folder<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
        if Path::new(path.as_ref()).exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir_all(&path)?;
        Ok(())
    }

    fn clean_and_setup_directories(&self) -> anyhow::Result<()> {
        if self.doc_list.len() == 0 {
            println!(
                "\nplease add files to source directory: {}\n",
                self.in_path.display()
            );
        }
        Self::clean_folder(&self.out_path)?;
        Self::copy_files(&self.template_dir_path, &self.out_path, "hbs")?;
        Ok(())
    }
    pub fn gen_book(&mut self) -> anyhow::Result<usize> {
        self.clean_and_setup_directories()?;
        info!("generating ePub for {} files", self.doc_list.len());

        match self.make_book_internal("Author Name", "My Book") {
            Err(e) => anyhow::bail!("Problem creating ebook: {:#?}", e),
            Ok(_) => Ok(self.doc_list.len())
        }
    }

    pub fn gen_website(&mut self) -> anyhow::Result<usize> {
        self.clean_and_setup_directories()?;
        info!("generating html for {} files", self.doc_list.len());
        for doc in &self.doc_list {
            let outpath = self.outpath(doc)?;
            Self::create_all_parent_dir(&outpath)?;
            doc.webgen(&self)?;
        }
        Ok(self.doc_list.len())
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_web() {
        let web = Web::new("markdown", "_website").expect("new web");
        assert_eq!(web.in_path, Path::new("markdown"));
        assert_eq!(web.out_path, Path::new("_website"));
    }
}
