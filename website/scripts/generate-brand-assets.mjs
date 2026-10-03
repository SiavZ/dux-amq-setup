// Rebuild current-brand icons from the single canonical vector. Historical
// upstream credits live in the footer and LICENSE, not in this new artwork.
import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import sharp from "sharp";

const root = new URL("../../", import.meta.url);
const svg = await readFile(new URL("assets/yaran-logo.svg", root));
const markPath = svg.toString().match(/<path d="([^"]+)"/)?.[1];
if (!markPath) throw new Error("Canonical Yaran mark has no path");
const app = new URL("crates/yaran-web/web/public/", root);
const site = new URL("website/public/", root);
const png = (size) => sharp(svg).resize(size, size).png().toBuffer();
await writeFile(new URL("assets/yaran-logo.png", root), await png(512));
await writeFile(new URL("yaran-logo.svg", app), svg);
await writeFile(new URL("yaran-logo.svg", site), svg);
await writeFile(new URL("favicon.svg", site), svg);
for (const [base, name, size] of [
  [app, "yaran-logo.png", 512], [app, "favicon.png", 128],
  [app, "icon-192.png", 192], [app, "icon-512.png", 512],
  [app, "icon-maskable-512.png", 512], [site, "yaran-logo.png", 512],
  [site, "favicon-96x96.png", 96], [site, "apple-touch-icon.png", 180],
  [site, "web-app-manifest-192x192.png", 192],
  [site, "web-app-manifest-512x512.png", 512],
]) {
  // The maskable version shrinks the foreground into the central safe area.
  const image = name.includes("maskable") || name.includes("web-app-manifest")
    ? await sharp(svg).resize(Math.round(size * 0.8)).extend({
        top: Math.floor(size * 0.1), bottom: Math.ceil(size * 0.1),
        left: Math.floor(size * 0.1), right: Math.ceil(size * 0.1),
        background: "#171717",
      }).resize(size, size).png().toBuffer()
    : await png(size);
  await writeFile(new URL(name, base), image);
}
// ICO permits PNG-encoded entries. Keep one 256px entry for modern browsers.
const icon = await png(256);
const header = Buffer.alloc(22);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(1, 4);
header.writeUInt16LE(1, 10);
header.writeUInt16LE(32, 12);
header.writeUInt32LE(icon.length, 14);
header.writeUInt32LE(22, 18);
await writeFile(new URL("favicon.ico", site), Buffer.concat([header, icon]));
const og = Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="630">
<rect width="1200" height="630" fill="#171717"/>
<path transform="translate(130 90) scale(.8)" d="${markPath}" fill="#e8b86d"/>
<text x="610" y="310" font-family="sans-serif" font-size="110" font-weight="bold" fill="#fafafa">Yaran</text>
<text x="610" y="375" font-family="sans-serif" font-size="25" fill="#d4d4d4">AI agents. One workspace.</text></svg>`);
await sharp(og).png().toFile(fileURLToPath(new URL("og.png", site)));
console.log("Generated Yaran app, website and social icons from assets/yaran-logo.svg");
