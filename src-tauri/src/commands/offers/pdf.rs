//! A small, dependency-free PDF writer for the booking documents (contract,
//! rider, invoice).
//!
//! Only what those documents need: A4 pages, the two standard PDF fonts
//! Helvetica and Helvetica-Bold (every reader ships them, nothing to embed),
//! WinAnsi encoding (French and the Western European accents, €, typographic
//! quotes), text with word wrap, rules and filled boxes. Text is measured with
//! the standard Helvetica metrics so wrapping and right alignment are exact.
//! Characters outside WinAnsi print as "?".

/// A4 in points.
pub const PAGE_W: f32 = 595.28;
pub const PAGE_H: f32 = 841.89;

pub type Rgb = (f32, f32, f32);

pub const INK: Rgb = (0.07, 0.07, 0.07);
pub const SUBTLE: Rgb = (0.42, 0.44, 0.47);
pub const RULE: Rgb = (0.82, 0.82, 0.80);
/// insrt.studio "sunset" (#ff5a1f).
pub const ACCENT: Rgb = (1.0, 0.353, 0.122);
pub const PANEL: Rgb = (0.965, 0.96, 0.95);

#[rustfmt::skip]
const HELV: [u16; 95] = [
    278,278,355,556,556,889,667,191,333,333,389,584,278,333,278,278, // space../
    556,556,556,556,556,556,556,556,556,556,278,278,584,584,584,556, // 0..?
    1015,667,667,722,722,667,611,778,722,278,500,667,556,833,722,778, // @..O
    667,778,722,667,611,722,667,944,667,667,611,278,278,278,469,556, // P.._
    333,556,556,500,556,556,278,556,556,222,222,500,222,833,556,556, // `..o
    556,556,333,500,278,556,500,722,500,500,500,334,260,334,584,     // p..~
];
#[rustfmt::skip]
const HELV_BOLD: [u16; 95] = [
    278,333,474,556,556,889,722,238,333,333,389,584,278,333,278,278,
    556,556,556,556,556,556,556,556,556,556,333,333,584,584,584,611,
    975,722,722,722,722,667,611,778,722,278,556,722,611,833,722,778,
    667,778,722,667,611,722,667,944,667,667,611,333,278,333,584,556,
    333,556,611,556,611,556,333,611,611,278,278,556,278,889,611,611,
    611,611,389,556,333,611,556,778,556,556,500,389,280,389,584,
];

/// WinAnsi (cp1252) byte for a char, `?` when it has none.
fn win_ansi(c: char) -> u8 {
    let u = c as u32;
    match c {
        '€' => 0x80,
        '‚' => 0x82,
        '„' => 0x84,
        '…' => 0x85,
        'Œ' => 0x8C,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        'œ' => 0x9C,
        'Ÿ' => 0x9F,
        '\u{202F}' => 0xA0, // narrow no-break space
        _ if (0x20..0x7F).contains(&u) || (0xA0..=0xFF).contains(&u) => u as u8,
        _ => b'?',
    }
}

/// Glyph width (1/1000 em) of a WinAnsi byte.
fn glyph_w(b: u8, bold: bool) -> u16 {
    let table = if bold { &HELV_BOLD } else { &HELV };
    let base = match b {
        0x20..=0x7E => b,
        0x80 => return 556,
        0x85 => return 1000,
        0x8C | 0x9C => return if bold { 1000 } else { 944 },
        0x91 | 0x92 | 0x82 => return 222,
        0x93 | 0x94 | 0x84 => return 333,
        0x95 => return 350,
        0x96 => return 556,
        0x97 => return 1000,
        0xA0 => b' ',
        0xC0..=0xC5 => b'A',
        0xC6 => return 1000,
        0xC7 => b'C',
        0xC8..=0xCB => b'E',
        0xCC..=0xCF => b'I',
        0xD1 => b'N',
        0xD2..=0xD6 | 0xD8 => b'O',
        0xD9..=0xDC => b'U',
        0xDD => b'Y',
        0xDF => return 611,
        0xE0..=0xE5 => b'a',
        0xE6 => return 889,
        0xE7 => b'c',
        0xE8..=0xEB => b'e',
        0xEC..=0xEF => b'i',
        0xF1 => b'n',
        0xF2..=0xF6 | 0xF8 => b'o',
        0xF9..=0xFC => b'u',
        0xFD | 0xFF => b'y',
        0xAB | 0xBB => return 556, // « »
        0xB0 => return 400,        // °
        _ => b'n',
    };
    table[(base - 0x20) as usize]
}

/// Width of `s` in points at `size`.
pub fn text_width(s: &str, size: f32, bold: bool) -> f32 {
    s.chars().map(|c| glyph_w(win_ansi(c), bold) as f32).sum::<f32>() * size / 1000.0
}

/// PDF string literal (WinAnsi bytes, octal escapes for anything non-ASCII).
fn pdf_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('(');
    for c in s.chars() {
        let b = win_ansi(c);
        match b {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(b as char);
            }
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("\\{:03o}", b)),
        }
    }
    out.push(')');
    out
}

/// Greedy word wrap to `width` points. Explicit newlines are kept; a single
/// word wider than the line is cut.
pub fn wrap(text: &str, size: f32, bold: bool, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if text_width(&candidate, size, bold) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // A word longer than the line: cut it.
            let mut chunk = String::new();
            for c in word.chars() {
                chunk.push(c);
                if text_width(&chunk, size, bold) > width && chunk.chars().count() > 1 {
                    let last = chunk.pop().unwrap();
                    lines.push(std::mem::take(&mut chunk));
                    chunk.push(last);
                }
            }
            line = chunk;
        }
        lines.push(line);
    }
    lines
}

/// A document being laid out top to bottom. `y` is the baseline cursor,
/// measured from the top of the page (converted to PDF coordinates on output).
pub struct Doc {
    title: String,
    pages: Vec<String>,
    cur: String,
    pub y: f32,
    pub margin: f32,
    /// Bottom limit for content (the footer lives below it).
    pub bottom: f32,
    /// Footer text, drawn on every page with "n / N".
    pub footer: String,
}

impl Doc {
    pub fn new(title: &str) -> Self {
        Doc {
            title: title.to_string(),
            pages: Vec::new(),
            cur: String::new(),
            y: 56.0,
            margin: 56.0,
            bottom: PAGE_H - 70.0,
            footer: String::new(),
        }
    }

    pub fn content_width(&self) -> f32 {
        PAGE_W - 2.0 * self.margin
    }

    pub fn new_page(&mut self) {
        let done = std::mem::take(&mut self.cur);
        self.pages.push(done);
        self.y = self.margin;
    }

    /// Start a new page when less than `h` points are left.
    pub fn ensure(&mut self, h: f32) {
        if self.y + h > self.bottom {
            self.new_page();
        }
    }

    fn color(&mut self, (r, g, b): Rgb, fill: bool) {
        let op = if fill { "rg" } else { "RG" };
        self.cur.push_str(&format!("{r:.3} {g:.3} {b:.3} {op}\n"));
    }

    /// Text at (x, top-based baseline y).
    pub fn text_at(&mut self, x: f32, y: f32, size: f32, bold: bool, color: Rgb, s: &str) {
        if s.is_empty() {
            return;
        }
        self.color(color, true);
        let font = if bold { "F2" } else { "F1" };
        self.cur.push_str(&format!(
            "BT /{font} {size:.1} Tf {x:.2} {:.2} Td {} Tj ET\n",
            PAGE_H - y,
            pdf_str(s)
        ));
    }

    pub fn text_right(&mut self, right: f32, y: f32, size: f32, bold: bool, color: Rgb, s: &str) {
        let w = text_width(s, size, bold);
        self.text_at(right - w, y, size, bold, color, s);
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        self.color(color, true);
        self.cur
            .push_str(&format!("{x:.2} {:.2} {w:.2} {h:.2} re f\n", PAGE_H - y - h));
    }

    pub fn hline(&mut self, x1: f32, x2: f32, y: f32, color: Rgb, width: f32) {
        self.color(color, false);
        self.cur.push_str(&format!(
            "{width:.2} w {x1:.2} {:.2} m {x2:.2} {:.2} l S\n",
            PAGE_H - y,
            PAGE_H - y
        ));
    }

    /// Wrapped paragraph at the cursor, page breaks included. Returns the
    /// number of lines written.
    pub fn paragraph(&mut self, x: f32, width: f32, size: f32, bold: bool, color: Rgb, s: &str) -> usize {
        let lead = size * 1.42;
        let lines = wrap(s, size, bold, width);
        for l in &lines {
            self.ensure(lead);
            self.y += lead;
            self.text_at(x, self.y - lead * 0.28, size, bold, color, l);
        }
        lines.len()
    }

    pub fn gap(&mut self, h: f32) {
        self.y += h;
    }

    /// The final file. The footer (with page numbers) is added to every page.
    pub fn finish(mut self) -> Vec<u8> {
        if !self.cur.is_empty() || self.pages.is_empty() {
            let last = std::mem::take(&mut self.cur);
            self.pages.push(last);
        }
        let total = self.pages.len();
        let footer = self.footer.clone();
        let margin = self.margin;
        let pages: Vec<String> = std::mem::take(&mut self.pages)
            .into_iter()
            .enumerate()
            .map(|(i, mut body)| {
                let mut f = Doc::new("");
                f.hline(margin, PAGE_W - margin, PAGE_H - 46.0, RULE, 0.5);
                f.text_at(margin, PAGE_H - 32.0, 7.5, false, SUBTLE, &footer);
                f.text_right(PAGE_W - margin, PAGE_H - 32.0, 7.5, false, SUBTLE, &format!("{} / {}", i + 1, total));
                body.push_str(&f.cur);
                body
            })
            .collect();

        // Objects: 1 catalog, 2 pages, 3 Helvetica, 4 Helvetica-Bold, 5 info,
        // then (page, content) pairs.
        let mut objs: Vec<String> = Vec::new();
        let kids: Vec<String> = (0..total).map(|i| format!("{} 0 R", 6 + 2 * i)).collect();
        objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), total));
        objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into());
        objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".into());
        objs.push(format!(
            "<< /Title {} /Producer (PiersCRM) /Creator (PiersCRM) >>",
            pdf_str(&self.title)
        ));
        for (i, body) in pages.iter().enumerate() {
            objs.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W:.2} {PAGE_H:.2}] \
                 /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
                7 + 2 * i
            ));
            objs.push(format!("<< /Length {} >>\nstream\n{}endstream", body.len(), body));
        }

        let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = Vec::with_capacity(objs.len());
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, o).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{}\n%%EOF\n",
                objs.len() + 1,
                xref
            )
            .as_bytes(),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_french_and_escapes() {
        assert_eq!(pdf_str("a(b)"), "(a\\(b\\))");
        assert_eq!(pdf_str("é€"), "(\\351\\200)");
        assert_eq!(pdf_str("日"), "(?)");
    }

    #[test]
    fn wraps_to_width() {
        let lines = wrap("le cachet est payable en deux fois", 10.0, false, 80.0);
        assert!(lines.len() > 1);
        for l in &lines {
            assert!(text_width(l, 10.0, false) <= 80.0);
        }
        assert_eq!(wrap("a\n\nb", 10.0, false, 100.0), vec!["a", "", "b"]);
    }

    #[test]
    fn produces_a_valid_looking_file() {
        let mut d = Doc::new("Test");
        d.footer = "PiersCRM".into();
        for _ in 0..120 {
            d.paragraph(56.0, 400.0, 10.0, false, INK, "Une ligne de texte accentuée, à répéter.");
        }
        let bytes = d.finish();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.starts_with("%PDF-1.4"));
        assert!(s.trim_end().ends_with("%%EOF"));
        assert!(s.contains("/Count 3") || s.contains("/Count 4"), "several pages");
        // xref offsets point at the objects.
        let xref_at: usize = s.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        assert_eq!(&bytes[xref_at..xref_at + 4], b"xref");
    }
}
