//! Pictures in the text: images, diagrams and formulas as inline DrawingML, each a relationship
//! of the part it is in to its media part. Sizes follow the image's `width` and `height`, else
//! its own size, never wider than the text; a formula takes the measures it was typeset with,
//! and sits on the line's baseline.

use adoc_render::{DiagramView, FormulaImage, ViewImage, ViewInline};

use crate::{
    body::{Format, Writer},
    inline::safe_url,
    media::Picture,
};

/// English Metric Units in a pixel at 96 to the inch, and in a point.
const EMU_PER_PIXEL: f64 = 9525.0;
const EMU_PER_POINT: f64 = 12700.0;
/// Twentieths of a point in a pixel at 96 to the inch.
const TWIPS_PER_PIXEL: f64 = 15.0;

const PICTURE: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";

/// A length in pixels: `120`, `120px` or `50%` of `whole`.
fn length(value: &str, whole: f64) -> Option<f64> {
    let value = value.trim();
    let pixels = match value.strip_suffix('%') {
        Some(percent) => percent.trim().parse::<f64>().ok()? * whole / 100.0,
        None => value
            .strip_suffix("px")
            .unwrap_or(value)
            .trim()
            .parse()
            .ok()?,
    };
    (pixels.is_finite() && pixels > 0.0).then_some(pixels)
}

impl Writer<'_> {
    /// A picture's size in EMUs: as `width` and `height` ask, the picture's proportions kept
    /// when only one is given, its own size otherwise, and no wider than the text.
    fn picture_size(
        &self,
        picture: &Picture,
        width: Option<&str>,
        height: Option<&str>,
    ) -> (i64, i64) {
        let text_width = f64::from(self.width) / TWIPS_PER_PIXEL;
        let (own_width, own_height) = (picture.width.max(1.0), picture.height.max(1.0));
        let width = width.and_then(|width| length(width, text_width));
        let height = height.and_then(|height| length(height, text_width));
        let (mut width, mut height) = match (width, height) {
            (Some(width), Some(height)) => (width, height),
            (Some(width), None) => (width, width * own_height / own_width),
            (None, Some(height)) => (height * own_width / own_height, height),
            (None, None) => (own_width, own_height),
        };
        if width > text_width {
            height *= text_width / width;
            width = text_width;
        }
        (
            (width * EMU_PER_PIXEL).round() as i64,
            (height * EMU_PER_PIXEL).round() as i64,
        )
    }

    /// The relationship id of a picture's media part from the part being written.
    fn picture_relationship(&mut self, index: usize) -> String {
        let pictures = &mut self.pictures[self.part as usize];
        if let Some((id, _)) = pictures.iter().find(|(_, known)| *known == index) {
            return id.clone();
        }
        let id = format!("rImage{}", pictures.len() + 1);
        pictures.push((id.clone(), index));
        id
    }

    /// A run holding `picture` at `size`, described by `alt`, lowered by `lowered` half-points,
    /// and leading to `link` when it is a safe address.
    fn drawing(
        &mut self,
        picture: &Picture,
        (width, height): (i64, i64),
        alt: &str,
        lowered: Option<i64>,
        link: Option<&str>,
    ) {
        let relationship = self.picture_relationship(picture.index);
        let link = link
            .filter(|link| safe_url(link))
            .map(|link| self.hyperlink(link));
        self.drawings += 1;
        let id = self.drawings.to_string();
        let name = format!("Picture {id}");
        let (width, height) = (width.to_string(), height.to_string());
        self.xml.open("w:r", &[]);
        if let Some(lowered) = lowered.filter(|lowered| *lowered != 0) {
            self.xml.open("w:rPr", &[]);
            self.xml
                .empty("w:position", &[("w:val", &(-lowered).to_string())]);
            self.xml.close("w:rPr");
        }
        self.xml.open("w:drawing", &[]);
        self.xml.open(
            "wp:inline",
            &[
                ("distT", "0"),
                ("distB", "0"),
                ("distL", "0"),
                ("distR", "0"),
            ],
        );
        self.xml
            .empty("wp:extent", &[("cx", &width), ("cy", &height)]);
        self.xml.empty(
            "wp:effectExtent",
            &[("l", "0"), ("t", "0"), ("r", "0"), ("b", "0")],
        );
        let description = [("id", id.as_str()), ("name", name.as_str()), ("descr", alt)];
        match &link {
            Some(link) => {
                self.xml.open("wp:docPr", &description);
                self.xml.empty("a:hlinkClick", &[("r:id", link)]);
                self.xml.close("wp:docPr");
            }
            None => self.xml.empty("wp:docPr", &description),
        }
        self.xml.open("wp:cNvGraphicFramePr", &[]);
        self.xml
            .empty("a:graphicFrameLocks", &[("noChangeAspect", "1")]);
        self.xml.close("wp:cNvGraphicFramePr");
        self.xml.open("a:graphic", &[]);
        self.xml.open("a:graphicData", &[("uri", PICTURE)]);
        self.xml.open("pic:pic", &[]);
        self.xml.open("pic:nvPicPr", &[]);
        self.xml.empty("pic:cNvPr", &description);
        self.xml.empty("pic:cNvPicPr", &[]);
        self.xml.close("pic:nvPicPr");
        self.xml.open("pic:blipFill", &[]);
        self.xml.empty("a:blip", &[("r:embed", &relationship)]);
        self.xml.open("a:stretch", &[]);
        self.xml.empty("a:fillRect", &[]);
        self.xml.close("a:stretch");
        self.xml.close("pic:blipFill");
        self.xml.open("pic:spPr", &[]);
        self.xml.open("a:xfrm", &[]);
        self.xml.empty("a:off", &[("x", "0"), ("y", "0")]);
        self.xml.empty("a:ext", &[("cx", &width), ("cy", &height)]);
        self.xml.close("a:xfrm");
        self.xml.open("a:prstGeom", &[("prst", "rect")]);
        self.xml.empty("a:avLst", &[]);
        self.xml.close("a:prstGeom");
        self.xml.close("pic:spPr");
        self.xml.close("pic:pic");
        self.xml.close("a:graphicData");
        self.xml.close("a:graphic");
        self.xml.close("wp:inline");
        self.xml.close("w:drawing");
        self.xml.close("w:r");
    }

    /// An image in the text, or, where it cannot be shown, its alternative text and why.
    pub(crate) fn image(&mut self, image: &ViewImage, format: Format, link: Option<&str>) {
        let alt = if image.alt.is_empty() {
            image.target.clone()
        } else {
            image.alt.clone()
        };
        if let Some(url) = &image.url {
            let shown = [ViewInline::Text(alt)];
            if safe_url(url) {
                self.external_link(url, &shown, format);
            } else {
                self.inlines(&shown, format);
            }
            self.warn(
                None,
                format!("the remote image {url} is not downloaded and is exported as a link to it"),
            );
            return;
        }
        let Some(path) = &image.path else {
            self.text(&format!("[{alt}]"), format);
            self.warn(
                None,
                format!(
                    "the image {} cannot be found and is exported as its alternative text",
                    image.target
                ),
            );
            return;
        };
        match self.media.load(path, &self.options.access) {
            Ok(picture) => {
                let size =
                    self.picture_size(&picture, image.width.as_deref(), image.height.as_deref());
                self.drawing(&picture, size, &alt, None, link);
            }
            Err(reason) => {
                self.text(&format!("[{alt}]"), format);
                let joined = if reason.contains(':') {
                    format!("{reason}; it is exported as its alternative text")
                } else {
                    format!("{reason} and is exported as its alternative text")
                };
                self.warn(None, format!("the image {} {joined}", image.target));
            }
        }
    }

    /// A diagram's or a block formula's drawing and its size, or `None` (reported when the
    /// drawing cannot be read) when it is not drawn.
    pub(crate) fn rendered(
        &mut self,
        rendered: &DiagramView,
        size: (Option<&str>, Option<&str>),
        what: &str,
    ) -> Option<(Picture, (i64, i64))> {
        let DiagramView::Svg(path) = rendered else {
            return None;
        };
        match self.media.load(path, &self.options.access) {
            Ok(picture) => {
                let size = self.picture_size(&picture, size.0, size.1);
                Some((picture, size))
            }
            Err(reason) => {
                self.warn(
                    None,
                    format!("{what} drawing {reason}; it is exported as its source"),
                );
                None
            }
        }
    }

    /// A drawing described by `alt`, as its own picture in the text.
    pub(crate) fn picture(&mut self, (picture, size): &(Picture, (i64, i64)), alt: &str) {
        self.drawing(picture, *size, alt, None, None);
    }

    /// An inline formula as its typeset image, on the line's baseline, else its source.
    pub(crate) fn inline_formula(
        &mut self,
        source: &str,
        rendered: Option<&FormulaImage>,
        format: Format,
    ) {
        let code = Format {
            style: Some("CodeChar"),
            ..format
        };
        let Some(formula) = rendered else {
            self.text(source, code);
            self.warn(
                None,
                "a formula that could not be typeset is exported as its source".to_owned(),
            );
            return;
        };
        match self.media.load(&formula.path, &self.options.access) {
            Ok(picture) => {
                let size = (
                    (f64::from(formula.width) * EMU_PER_POINT).round() as i64,
                    (f64::from(formula.height) * EMU_PER_POINT).round() as i64,
                );
                // Half-points below the baseline: the formula's depth.
                let depth = (f64::from(formula.height - formula.baseline) * 2.0).round() as i64;
                self.drawing(&picture, size, source, Some(depth), None);
            }
            Err(reason) => {
                self.text(source, code);
                self.warn(
                    None,
                    format!("a formula's drawing {reason}; it is exported as its source"),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::length;

    #[test]
    fn lengths_are_pixels_or_shares_of_the_width() {
        assert_eq!(length("120", 600.0), Some(120.0));
        assert_eq!(length(" 120px ", 600.0), Some(120.0));
        assert_eq!(length("50%", 600.0), Some(300.0));
        assert_eq!(length("0", 600.0), None);
        assert_eq!(length("-3", 600.0), None);
        assert_eq!(length("wide", 600.0), None);
        assert_eq!(length("NaN", 600.0), None);
    }
}
