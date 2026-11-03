// Minimal Markdown for notebook text: headings, paragraphs, lists, bold,
// italics and inline code. Input is escaped first, so cell text can never
// inject markup.

function escape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function inline(s: string): string {
  return escape(s)
    .replace(/`([^`]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^*])\*([^*]+)\*/g, "$1<em>$2</em>");
}

export function renderMarkdown(text: string): string {
  const out: string[] = [];
  for (const block of text.trim().split(/\n\s*\n/)) {
    const lines = block.split("\n");
    const heading = /^(#{1,3})\s+(.*)$/.exec(lines[0] ?? "");
    if (heading && lines.length === 1) {
      const level = heading[1]!.length + 1;
      out.push(`<h${level}>${inline(heading[2]!)}</h${level}>`);
    } else if (lines.every((l) => /^\s*[-*]\s+/.test(l))) {
      out.push(`<ul>${lines.map((l) => `<li>${inline(l.replace(/^\s*[-*]\s+/, ""))}</li>`).join("")}</ul>`);
    } else if (heading) {
      const level = heading[1]!.length + 1;
      out.push(`<h${level}>${inline(heading[2]!)}</h${level}>`, `<p>${lines.slice(1).map(inline).join("<br>")}</p>`);
    } else {
      out.push(`<p>${lines.map(inline).join("<br>")}</p>`);
    }
  }
  return out.join("");
}
