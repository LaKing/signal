#!/usr/bin/env node
// Writes THIRD-PARTY-NOTICES.md (ADR 0031 §2): the crates linked into the
// shipped binaries (Signal.app's `signal-app` and the `signal` CLI, built as
// scripts/bundle-app.sh builds them: `--features signal-cli/gpu`, for
// aarch64-apple-darwin), each with its version, licence expression and
// repository, and the licence and notice texts in its source directory
// (LICENSE*, LICENCE*, COPYING*, NOTICE*), each text once.
//
//   node scripts/third-party-notices.mjs                 write the file
//   node scripts/third-party-notices.mjs --check         fail when it is stale
//   ... --metadata FILE   read `cargo metadata` output from FILE (tests)
//   ... --out FILE        write or check FILE instead
//
// Reads `cargo metadata --offline --locked` (Cargo.lock and the crate sources
// ./install.sh or `cargo fetch --locked` downloaded); never the network.
// Linked means: reached from the two binaries' packages over normal
// dependencies only (no dev- or build-dependencies), on that target only,
// and not through a proc-macro (a proc-macro runs in the compiler and is not
// linked). That set is `cargo tree -e normal,no-proc-macro --target …`,
// whose feature resolution is the build's: `cargo metadata` resolves
// features for every platform at once (wgpu's Vulkan and GLES backends would
// appear), so it supplies each crate's details, not the set. Signal's own
// crates are GPL-3.0-or-later and not listed. Both modes fail when a linked
// crate's licence is outside the allowed set below. Node built-ins only.

import { readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('..', import.meta.url));
const TARGET = 'aarch64-apple-darwin';
const FEATURES = 'signal-cli/gpu';
const BINARIES = ['signal-app', 'signal-cli'];

// ADR 0031 §2: permissive licences, and MPL-2.0 with a note. A crate's
// expression must allow one choice made of these alone.
const ALLOWED = new Set([
  '0BSD', 'Apache-2.0', 'BSD-2-Clause', 'BSD-3-Clause', 'ISC', 'MIT', 'MIT-0',
  'MPL-2.0', 'Unicode-3.0', 'Unicode-DFS-2016', 'Zlib',
]);
const NOTED = 'MPL-2.0';
const TEXT = /^(LICEN[CS]E|COPYING|NOTICE)/i;

function die(message) {
  process.stderr.write(`third-party-notices: ${message}\n`);
  process.exit(1);
}

const args = process.argv.slice(2);
let check = false;
let metadataFile = null;
let out = join(ROOT, 'THIRD-PARTY-NOTICES.md');
while (args.length > 0) {
  const arg = args.shift();
  if (arg === '--check') check = true;
  else if (arg === '--metadata' && args.length > 0) metadataFile = args.shift();
  else if (arg === '--out' && args.length > 0) out = args.shift();
  else die(`unknown argument: ${arg}`);
}

function cargo(argv) {
  // As scripts/bundle-app.sh finds cargo: $CARGO, PATH, then Homebrew's keg-only rustup.
  for (const bin of [process.env.CARGO, 'cargo', '/opt/homebrew/opt/rustup/bin/cargo']) {
    if (!bin) continue;
    try {
      return execFileSync(bin, argv, { cwd: ROOT, encoding: 'utf8', maxBuffer: 1 << 28, stdio: ['ignore', 'pipe', 'inherit'] });
    } catch (error) {
      if (error.code !== 'ENOENT') die(`cargo ${argv[0]} failed (run ./install.sh or cargo fetch --locked)`);
    }
  }
  die('cargo not found');
}

/** Whether an SPDX expression (`/` read as OR, as older manifests write it) allows a choice inside ALLOWED. */
function allowed(expression) {
  const tokens = expression.replace(/\//g, ' OR ').match(/\(|\)|[^\s()]+/g) ?? [];
  let at = 0;
  const any = () => {
    let ok = all();
    while (tokens[at] === 'OR') { at++; ok = all() || ok; }
    return ok;
  };
  const all = () => {
    let ok = one();
    while (tokens[at] === 'AND') { at++; ok = one() && ok; }
    return ok;
  };
  const one = () => {
    if (tokens[at] === '(') {
      at++;
      const ok = any();
      if (tokens[at++] !== ')') throw new Error('unbalanced parentheses');
      return ok;
    }
    const id = tokens[at++];
    // An exception only adds permissions (e.g. Apache-2.0 WITH LLVM-exception).
    if (tokens[at] === 'WITH') at += 2;
    return ALLOWED.has(id);
  };
  try {
    const ok = any();
    return at === tokens.length && ok;
  } catch {
    return false;
  }
}

const metadata = JSON.parse(metadataFile
  ? readFileSync(metadataFile, 'utf8')
  : cargo(['metadata', '--offline', '--locked', '--format-version', '1', '--filter-platform', TARGET, '--features', FEATURES]));
const workspace = new Set(metadata.workspace_members);

// `name vVERSION`, with ` (path)` for path packages and ` (*)` for repeats.
const tree = cargo(['tree', '--offline', '--locked', ...BINARIES.flatMap((b) => ['-p', b]), '--features', FEATURES,
  '-e', 'normal,no-proc-macro', '--target', TARGET, '--prefix', 'none', '--format', '{p}']);
const linked = new Map();
for (const line of tree.split('\n')) {
  const found = /^(\S+) v(\S+)/.exec(line);
  if (!found) continue;
  const [, name, version] = found;
  const p = metadata.packages.find((q) => q.name === name && q.version === version);
  if (!p) die(`cargo tree names ${name} ${version}, which cargo metadata does not`);
  if (!workspace.has(p.id)) linked.set(p.id, p);
}

// Code-point order, so the file is the same under every locale.
const order = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
const crates = [...linked.values()].sort((a, b) => order(a.name, b.name) || order(a.version, b.version));
const refused = crates.filter((p) => !p.license || !allowed(p.license));
if (refused.length > 0) {
  die(`licence outside ADR 0031's allowed set:\n${refused.map((p) => `  ${p.name} ${p.version}: ${p.license ?? '(no licence expression)'}`).join('\n')}`);
}

// The texts, each once, in the order of the first crate that carries it.
const texts = new Map();
const files = new Map();
for (const p of crates) {
  const dir = dirname(p.manifest_path);
  const names = readdirSync(dir).filter((f) => TEXT.test(f) && statSync(join(dir, f)).isFile()).sort();
  files.set(p.id, names);
  for (const name of names) {
    const text = readFileSync(join(dir, name), 'utf8').replace(/\r\n?/g, '\n').trimEnd();
    const key = createHash('sha256').update(text).digest('hex');
    if (!texts.has(key)) texts.set(key, { text, users: [] });
    texts.get(key).users.push(`${p.name} ${p.version} (${name})`);
  }
}

const crateLink = (p) => `https://crates.io/crates/${p.name}/${p.version}`;
const cell = (s) => s.replace(/\|/g, '\\|');
const fence = (text) => {
  let f = '```';
  while (text.includes(f)) f += '`';
  return f;
};
const noted = crates.filter((p) => p.license.split(/[\s()/]+/).includes(NOTED));
const unlicensed = crates.filter((p) => files.get(p.id).length === 0);

const lines = [
  '# Third-party notices',
  '',
  '<!-- Generated by scripts/third-party-notices.mjs from Cargo.lock: do not edit by hand. -->',
  '',
  'Signal is licensed under the GNU General Public License, version 3 or (at your option) any later version (`GPL-3.0-or-later`, the file `LICENSE`).',
  `Its release binaries for ${TARGET} (Signal.app and the \`signal\` command) link the ${crates.length} crates below, each under its own licence.`,
  'Where a crate offers a choice of licences, Signal uses it under one that is compatible with the GPL.',
  'The licence and notice texts each crate ships in its source are reproduced once each at the end, with the crates that carry them.',
  '',
  '## MPL-2.0',
  '',
];
if (noted.length === 0) {
  lines.push('No linked crate is under the Mozilla Public License 2.0.');
} else {
  lines.push('These crates are covered by the Mozilla Public License 2.0. Signal uses them unmodified; their source code form is available at the addresses below (MPL-2.0 §3.2).', '');
  for (const p of noted) lines.push(`- ${p.name} ${p.version} (\`${p.license}\`): ${crateLink(p)}${p.repository ? `, ${p.repository}` : ''}`);
}
lines.push('', '## Crates', '', '| Crate | Version | Licence | Repository |', '|---|---|---|---|');
for (const p of crates) lines.push(`| ${p.name} | ${p.version} | ${cell(p.license)} | ${p.repository ?? crateLink(p)} |`);
if (unlicensed.length > 0) {
  lines.push('', 'These crates ship no licence file in their source; their licence is the expression above:', '');
  for (const p of unlicensed) lines.push(`- ${p.name} ${p.version} (\`${p.license}\`)`);
}
lines.push('', '## Licence texts');
let n = 0;
for (const { text, users } of texts.values()) {
  const f = fence(text);
  lines.push('', `### Text ${++n}`, '', `Carried by: ${users.join(', ')}.`, '', `${f}text`, text, f);
}
const notices = `${lines.join('\n')}\n`;

if (check) {
  let committed = null;
  try {
    committed = readFileSync(out, 'utf8');
  } catch {
    die(`${out} is missing: run node scripts/third-party-notices.mjs`);
  }
  if (committed !== notices) die(`${out} is stale against Cargo.lock: run node scripts/third-party-notices.mjs`);
  process.stdout.write(`third-party-notices: ${out} is current (${crates.length} crates, ${texts.size} texts)\n`);
} else {
  writeFileSync(out, notices);
  process.stdout.write(`third-party-notices: wrote ${out} (${crates.length} crates, ${texts.size} texts)\n`);
}
