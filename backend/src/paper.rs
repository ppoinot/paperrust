pub struct PaperSpec<'a> {
    pub kicker: Option<&'a str>,
    pub title: &'a str,
    pub correspondent: &'a str,
    pub date: &'a str,
    pub r#ref: Option<&'a str>,
    pub lines: &'a [String],
    pub footer: Option<&'a str>,
    pub stamp: Option<&'a str>,
    pub accent: Option<&'a str>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn render_paper_svg(spec: PaperSpec<'_>) -> String {
    let accent = spec.accent.unwrap_or("#2c4a3e");
    let mut body = String::new();
    for (i, line) in spec.lines.iter().enumerate() {
        body.push_str(&format!(
            "<text x=\"36\" y=\"{}\" font-size=\"10.5\" font-family=\"Georgia, serif\" fill=\"#2c2822\">{}</text>",
            168 + i * 15,
            esc(line)
        ));
    }
    let stamp = if let Some(st) = spec.stamp {
        format!(
            "<g transform=\"translate(292 420) rotate(-12)\"><circle cx=\"0\" cy=\"0\" r=\"38\" fill=\"none\" stroke=\"{a}\" stroke-width=\"2.2\" opacity=\"0.55\"/><circle cx=\"0\" cy=\"0\" r=\"32\" fill=\"none\" stroke=\"{a}\" stroke-width=\"0.8\" opacity=\"0.45\"/><text text-anchor=\"middle\" y=\"-4\" font-size=\"8\" fill=\"{a}\" font-family=\"Georgia, serif\" opacity=\"0.7\">{st}</text><text text-anchor=\"middle\" y=\"10\" font-size=\"7\" fill=\"{a}\" font-family=\"Georgia, serif\" opacity=\"0.7\">{dt}</text></g>",
            a = accent,
            st = esc(st),
            dt = esc(spec.date)
        )
    } else {
        String::new()
    };
    let kicker = esc(&spec.kicker.unwrap_or(spec.correspondent).to_uppercase());
    let ref_part = spec
        .r#ref
        .map(|r| format!("  ·  {}", esc(r)))
        .unwrap_or_default();
    let footer = esc(spec.footer.unwrap_or("Paperrust · archive"));
    let title = esc(spec.title);
    let corr = esc(spec.correspondent);
    let date = esc(spec.date);
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 420 594\" width=\"420\" height=\"594\">\n\
  <rect width=\"420\" height=\"594\" fill=\"#f6f1e4\"/>\n\
  <rect x=\"18\" y=\"18\" width=\"384\" height=\"558\" fill=\"#fbf7ee\" stroke=\"#d7d0c3\"/>\n\
  <rect x=\"18\" y=\"18\" width=\"6\" height=\"558\" fill=\"{accent}\"/>\n\
  <text x=\"36\" y=\"48\" font-size=\"9\" letter-spacing=\"2.4\" fill=\"{accent}\" font-family=\"ui-sans-serif, system-ui\" font-weight=\"600\">{kicker}</text>\n\
  <text x=\"36\" y=\"78\" font-size=\"20\" font-family=\"Georgia, serif\" fill=\"#1c1915\">{title}</text>\n\
  <text x=\"36\" y=\"102\" font-size=\"11\" fill=\"#6b6458\" font-family=\"ui-sans-serif, system-ui\">{corr}</text>\n\
  <text x=\"36\" y=\"120\" font-size=\"10\" fill=\"#8a8274\" font-family=\"ui-sans-serif, system-ui\">{date}{ref_part}</text>\n\
  <line x1=\"36\" y1=\"136\" x2=\"384\" y2=\"136\" stroke=\"#e2dacb\"/>\n\
  {body}\n\
  {stamp}\n\
  <text x=\"36\" y=\"552\" font-size=\"8.5\" fill=\"#9a9284\" font-family=\"ui-sans-serif, system-ui\">{footer}</text>\n\
</svg>"
    )
}
