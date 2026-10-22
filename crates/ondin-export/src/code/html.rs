//! The plain HTML/CSS emitter: `index.html`, one `styles.css`, `assets/` and
//! `report.txt`. No components — an instance is written out as its layers.

use super::{CodeExport, CodeFile, walk};
use ondin_core::{Document, NodeId, Resolved};

/// Rules every page starts with: no default margins, and sizes that include
/// padding and borders, which is how the model measures a box.
const BASE_CSS: &str = "\
*, *::before, *::after {
  box-sizing: border-box;
}
body {
  margin: 0;
}
";

/// `origins` as a page, in the order given (later on top).
pub fn html_of(doc: &Document, res: &Resolved, origins: &[NodeId]) -> CodeExport {
    let page = walk::page(doc, res, origins);

    let mut body = String::new();
    page.root.html(&mut body, 1);
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

    let mut files = vec![CodeFile {
        path: "index.html".into(),
        bytes: index.into_bytes(),
    }];
    if let Some(tokens) = page.tokens {
        files.push(CodeFile {
            path: "tokens.css".into(),
            bytes: tokens.into_bytes(),
        });
    }
    files.push(CodeFile {
        path: "styles.css".into(),
        bytes: styles.into_bytes(),
    });
    files.extend(
        page.assets
            .into_iter()
            .map(|(path, bytes)| CodeFile { path, bytes }),
    );
    files.push(CodeFile {
        path: "report.txt".into(),
        bytes: page.report.text().into_bytes(),
    });
    CodeExport {
        files,
        approximated: page.report.approximated,
        positioned: page.report.positioned,
    }
}
