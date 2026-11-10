// Search index for the docs search box in Docs.astro: one entry per section
// (page intro, then each h2/h3), plain text, built at build time.
const strip = (html: string) =>
  html.replace(/<[^>]+>/g, " ").replace(/&#x([0-9a-f]+);/gi, (_, h) => String.fromCodePoint(parseInt(h, 16)))
    .replace(/&(amp|lt|gt|quot);/g, (_, e) => ({ amp: "&", lt: "<", gt: ">", quot: '"' })[e as string]!)
    .replace(/\s+/g, " ").trim();

export async function GET() {
  const pages = Object.values(import.meta.glob("./*.md", { eager: true })) as any[];
  const out = [];
  for (const p of pages) {
    const url = p.url.replace(/\/$/, "");
    // split() with a capture group yields [intro, id, heading+body, id, heading+body, ...]
    const parts = (await p.compiledContent()).split(/<h[23] id="([^"]+)">/);
    out.push({ url, title: p.frontmatter.title, text: strip(parts[0]) });
    for (let i = 1; i < parts.length; i += 2) {
      const [head, body = ""] = parts[i + 1].split(/<\/h[23]>/);
      out.push({ url: `${url}#${parts[i]}`, title: `${p.frontmatter.title} › ${strip(head)}`, text: strip(body) });
    }
  }
  return new Response(JSON.stringify(out), { headers: { "content-type": "application/json" } });
}
