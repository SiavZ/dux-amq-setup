import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const read = (relative: string) => readFileSync(new URL(relative, import.meta.url), "utf8");

describe("historical upstream terminal captures", () => {
  it.each(["Asciinema", "Manifesto"])("%s labels the original dux capture rather than claiming current Yaran", (component) => {
    const source = read(`../components/${component}.astro`);
    expect(source).toContain('src="/dux-screenshot.svg"');
    expect(source).not.toContain("/yaran-screenshot.svg");
    expect(source).toContain("Historical upstream dux terminal capture, before the Yaran rebrand");
  });

  it("retains the actual inherited recording identity with a historical label", () => {
    const source = read("../components/Asciinema.astro");
    expect(source).toContain('castId = "IvqL89rXvwCzvSxQ"');
    expect(source).toContain('title = "Historical upstream dux recording"');
    expect(source).not.toContain("yaran in action");
    expect(source).not.toContain(">live recording<");
  });

  it("copies the archived screenshot under its original filename", () => {
    const script = read("../../scripts/copy-install.mjs");
    expect(script).toContain('resolve(repoRoot, "assets", "dux-screenshot.svg")');
    expect(script).toContain('resolve(publicDir, "dux-screenshot.svg")');
    expect(script).not.toContain("yaran-screenshot.svg");
    expect(read("../../../assets/dux-screenshot.svg")).toContain("<svg");
  });
});
