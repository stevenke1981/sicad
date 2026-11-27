// Generates src/api/generated/*.d.ts from the JSON Schemas (ADR 0008):
//   promptcad.d.ts     <- schemas/promptcad.schema.json
//   core-request.d.ts  <- schemas/cad2/core-request.schema.json
//   core-response.d.ts <- schemas/cad2/core-response.schema.json
// Output is deterministic (fixed Prettier options, LF line endings, trailing newline) so CI
// can regenerate and run `git diff --exit-code`.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { compile } from 'json-schema-to-typescript';

const here = dirname(fileURLToPath(import.meta.url));
const schemasDir = resolve(here, '../../schemas');
const outDir = resolve(here, '../src/api/generated');

const banner = (source, regen) =>
  [
    '/* eslint-disable */',
    '/**',
    ` * AUTO-GENERATED FILE: do not edit — generated from ${source}.`,
    ` * Regenerate with \`npm run gen:types\` (in web/) after ${regen}.`,
    ' */',
  ].join('\n');

/**
 * Pydantic puts a `title` on every property, which json-schema-to-typescript turns into a
 * named alias per field (Kind1, Width2, ...). Keep titles only on the top-level `$defs`
 * entries so each model becomes one named type and its fields are inlined.
 */
function stripNestedTitles(root) {
  const strip = (node, keepTitle) => {
    if (Array.isArray(node)) return node.map((item) => strip(item, false));
    if (node === null || typeof node !== 'object') return node;
    const out = {};
    for (const key of Object.keys(node)) {
      if (key === 'title' && !keepTitle) continue;
      if (key === 'properties') {
        out.properties = Object.fromEntries(
          Object.entries(node.properties).map(([name, value]) => [name, strip(value, false)]),
        );
        continue;
      }
      out[key] = strip(node[key], false);
    }
    return out;
  };
  const { $defs = {}, ...rest } = root;
  const defs = Object.fromEntries(
    Object.entries($defs).map(([name, value]) => [name, strip(value, true)]),
  );
  return { ...strip(rest, true), $defs: defs };
}

/**
 * json-schema-to-typescript turns a `$ref` with sibling keywords (e.g. `description`) into
 * numbered duplicate types (Resolved1, CadDocument21, ...). Keep only the `$ref` itself.
 */
function stripRefSiblings(node) {
  if (Array.isArray(node)) return node.map(stripRefSiblings);
  if (node === null || typeof node !== 'object') return node;
  if (typeof node.$ref === 'string') return { $ref: node.$ref };
  return Object.fromEntries(Object.entries(node).map(([k, v]) => [k, stripRefSiblings(v)]));
}

const options = (schemaPath, bannerComment) => ({
  bannerComment,
  unreachableDefinitions: true,
  declareExternallyReferenced: true,
  additionalProperties: false,
  enableConstEnums: false,
  inferStringEnumKeysFromValues: false,
  strictIndexSignatures: false,
  unknownAny: true,
  ignoreMinAndMaxItems: true,
  format: true,
  cwd: dirname(schemaPath),
  style: {
    printWidth: 100,
    singleQuote: true,
    semi: true,
    tabWidth: 2,
    useTabs: false,
    trailingComma: 'all',
    bracketSpacing: true,
    endOfLine: 'lf',
  },
});

async function generate({ schemaFile, outFile, name, regen, prepare }) {
  const schemaPath = resolve(schemasDir, schemaFile);
  const raw = JSON.parse(await readFile(schemaPath, 'utf8'));
  let ts = await compile(
    prepare(raw),
    name,
    options(schemaPath, banner(`schemas/${schemaFile}`, regen)),
  );
  ts = ts.replace(/\r\n?/g, '\n').replace(/\s+$/, '') + '\n';
  const outPath = resolve(outDir, outFile);
  await mkdir(dirname(outPath), { recursive: true });
  await writeFile(outPath, ts, { encoding: 'utf8' });
  console.log(`Wrote ${outPath}`);
}

await generate({
  schemaFile: 'promptcad.schema.json',
  outFile: 'promptcad.d.ts',
  name: 'PromptCADSchema',
  regen: 'running scripts/export_schema.py',
  prepare: stripNestedTitles,
});
// The two core schemas are compiled separately (each is self-contained; PatchOrigin appears
// in both) so they never produce conflicting exports in one module.
await generate({
  schemaFile: 'cad2/core-request.schema.json',
  outFile: 'core-request.d.ts',
  name: 'CoreRequest',
  regen: 'changing the Rust core API (cargo run -- schema)',
  prepare: stripRefSiblings,
});
await generate({
  schemaFile: 'cad2/core-response.schema.json',
  outFile: 'core-response.d.ts',
  name: 'CoreResponse',
  regen: 'changing the Rust core API (cargo run -- schema)',
  prepare: stripRefSiblings,
});
