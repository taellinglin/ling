// One-off generator: per-crate icon.svg, README.md, Cargo.toml `readme` field,
// and a minimal Ling.toml (so `lingfu publish` works for every crate too).
// Run from repo root: node scripts/gen_crate_assets.js
const fs = require("fs");
const path = require("path");

const ROOT = process.cwd();
const CRATES_DIR = path.join(ROOT, "crates");

// name -> [up-triangle color, down-triangle color] — same brand structure
// (navy tile, spectrum ring, teal/rose hexagram) as editors/vscode-ling's
// icon, just with the hexagram's two accent colors varied per crate so the
// family stays visually cohesive but each crate reads as distinct.
const ACCENTS = {
  "ling-core": ["#00e0cc", "#ff1040"],
  "ling-lex": ["#ffb000", "#4040ff"],
  "ling-polyglot": ["#a020f0", "#ffd700"],
  "ling-ai": ["#8020ff", "#00e0ff"],
  "ling-icon": ["#c0c0e0", "#ffcc33"],
  "ling-crypto": ["#ffd700", "#b0002a"],
  "ling-audio": ["#00e0ff", "#ff30c0"],
  "ling-net": ["#2050ff", "#20ff90"],
  "ling-http": ["#30a0ff", "#ff8020"],
  "ling-mir": ["#8090b0", "#ffb000"],
  "ling-fu": ["#ffd700", "#20e070"],
  "ling-graphics": ["#00e0cc", "#a040ff"],
  "ling-wasm": ["#7040ff", "#3090ff"],
  "ling-ast": ["#30d060", "#ffcc33"],
  "ling-codegen": ["#ff8020", "#4070a0"],
  "ling-runtime": ["#ff3050", "#00c0b0"],
  "ling-ui": ["#00c0b0", "#8090a0"],
  "ling-game": ["#ff1040", "#ffd700"],
  "ling-macro": ["#9040ff", "#ff40a0"],
  "ling-mic": ["#00e0ff", "#90a0b0"],
  "ling-physics": ["#2060ff", "#ff9020"],
  "ling-py": ["#2b5b84", "#ffd43b"],
  "ling-music": ["#ff40a0", "#8040ff"],
  "ling-gpu": ["#30ff70", "#00807a"],
  "ling-animation": ["#ff8020", "#00c0b0"],
  "ling-input": ["#00e0cc", "#ff1040"],
  "ling-web": ["#2080ff", "#ffd700"],
};

function iconSvg(upColor, downColor) {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" width="128" height="128">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#10203f"/>
      <stop offset="1" stop-color="#070b18"/>
    </linearGradient>
    <linearGradient id="spectrum" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0"   stop-color="#ff1040"/>
      <stop offset="0.35" stop-color="#ffb000"/>
      <stop offset="0.6" stop-color="#40e020"/>
      <stop offset="0.8" stop-color="#00e0cc"/>
      <stop offset="1"   stop-color="#0070ff"/>
    </linearGradient>
    <filter id="glow" x="-40%" y="-40%" width="180%" height="180%">
      <feGaussianBlur stdDeviation="1.6" result="b"/>
      <feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge>
    </filter>
    <filter id="glowSoft" x="-60%" y="-60%" width="220%" height="220%">
      <feGaussianBlur stdDeviation="2.6" result="b"/>
      <feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge>
    </filter>
  </defs>
  <rect x="3" y="3" width="122" height="122" rx="26" fill="url(#bg)"/>
  <rect x="3.5" y="3.5" width="121" height="121" rx="25.5" fill="none" stroke="#ffffff" stroke-width="1" opacity="0.08"/>
  <circle cx="64" cy="64" r="50" fill="none" stroke="url(#spectrum)" stroke-width="2.4" opacity="0.9" filter="url(#glow)"/>
  <circle cx="64" cy="64" r="41" fill="none" stroke="#0070ff" stroke-width="1"   opacity="0.45"/>
  <polygon points="64,24 96,80 32,80" fill="none" stroke="${upColor}" stroke-width="4"
           stroke-linejoin="round" opacity="0.95" filter="url(#glow)"/>
  <polygon points="64,104 32,48 96,48" fill="none" stroke="${downColor}" stroke-width="4"
           stroke-linejoin="round" opacity="0.95" filter="url(#glow)"/>
  <circle cx="64" cy="64" r="8.5" fill="none" stroke="${upColor}" stroke-width="2" filter="url(#glowSoft)"/>
  <circle cx="64" cy="64" r="3.6" fill="#ffffff" opacity="0.97" filter="url(#glowSoft)"/>
</svg>
`;
}

function readCargoField(text, field) {
  const re = new RegExp(`^${field}\\s*=\\s*"([^"]*)"`, "m");
  const m = text.match(re);
  return m ? m[1] : "";
}

const names = fs
  .readdirSync(CRATES_DIR)
  .filter((n) => n !== "ling-kernel")
  .filter((n) => fs.statSync(path.join(CRATES_DIR, n)).isDirectory());

for (const name of names) {
  const dir = path.join(CRATES_DIR, name);
  const cargoPath = path.join(dir, "Cargo.toml");
  if (!fs.existsSync(cargoPath)) continue;
  let cargo = fs.readFileSync(cargoPath, "utf8");

  const version = readCargoField(cargo, "version");
  const description = readCargoField(cargo, "description");
  const license = readCargoField(cargo, "license") || "Apache-2.0 OR MIT";
  const repository =
    readCargoField(cargo, "repository") || "https://github.com/taellinglin/ling";

  const accents = ACCENTS[name] || ["#00e0cc", "#ff1040"];

  // 1) icon.svg
  const imagesDir = path.join(dir, "images");
  fs.mkdirSync(imagesDir, { recursive: true });
  fs.writeFileSync(path.join(imagesDir, "icon.svg"), iconSvg(accents[0], accents[1]));

  // 2) README.md — prepend the icon if a README already exists (ling-input),
  // otherwise write a clean minimal one.
  const readmePath = path.join(dir, "README.md");
  const iconLine = `![${name} icon](images/icon.svg)\n\n`;
  if (fs.existsSync(readmePath)) {
    const existing = fs.readFileSync(readmePath, "utf8");
    if (!existing.startsWith("![")) {
      fs.writeFileSync(readmePath, iconLine + existing);
    }
  } else {
    const body = `${iconLine}# ${name}\n\n${description}\n\nPart of the [Ling](https://ling-lang.org) programming language ecosystem. See the [main repository](${repository}) for docs, examples, and the full workspace.\n`;
    fs.writeFileSync(readmePath, body);
  }

  // 3) Cargo.toml: add `readme = "README.md"` right after `license` if absent.
  if (!/^readme\s*=/m.test(cargo)) {
    cargo = cargo.replace(
      /^(license\s*=\s*"[^"]*")$/m,
      `$1\nreadme = "README.md"`
    );
    fs.writeFileSync(cargoPath, cargo);
  }

  // 4) Ling.toml — minimal manifest so `lingfu publish` works too. Version is
  // a placeholder; the real publish always passes `--version` explicitly
  // from the just-bumped Cargo.toml, so this never needs to stay in sync.
  const lingToml = `[package]
name = "${name}"
version = "${version}"
description = "${description.replace(/"/g, '\\"')}"
license = "${license}"
repository = "${repository}"
homepage = "https://ling-lang.org"
authors = ["Ling Lin <taellinglin@gmail.com>", "Sanny Lin <SannyLing53@gmail.com>"]
`;
  fs.writeFileSync(path.join(dir, "Ling.toml"), lingToml);

  console.log(`done: ${name}`);
}
console.log(`\n${names.length} crates processed.`);
