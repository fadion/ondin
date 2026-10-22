//! The plain HTML/CSS emitter: `index.html`, one `styles.css`, `assets/` and
//! `report.txt`. No components — an instance is written out as its layers.

use super::dom::Syntax;
use super::{BASE_CSS, CodeExport, CodeFile, walk};
use ondin_core::{Document, NodeId, Resolved};

/// `origins` as a page, in the order given (later on top).
pub fn html_of(doc: &Document, res: &Resolved, origins: &[NodeId]) -> CodeExport {
    let page = walk::page(doc, res, origins, false);

    let mut body = String::new();
    page.root.markup(&mut body, 1, Syntax::Html);
    let tokens_link = if page.tokens.is_some() {
        "<link rel=\"stylesheet\" href=\"tokens.css\">\n"
    } else {
        ""
    };
    let index = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Ondin export</title>\n{tokens_link}<link rel=\"stylesheet\" href=\"styles.css\">\n\
         </head>\n<body>\n{body}</body>\n</html>\n"
    );

    let mut styles = BASE_CSS.to_string();
    page.root.css(&mut styles);

    let files = vec![
        CodeFile {
            path: "index.html".into(),
            bytes: index.into_bytes(),
        },
        CodeFile {
            path: "styles.css".into(),
            bytes: styles.into_bytes(),
        },
    ];
    super::finish(files, page)
}
