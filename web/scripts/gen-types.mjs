// Generates src/api/generated/promptcad.d.ts from ../schemas/promptcad.schema.json (ADR 0008).
// Output is deterministic (fixed Prettier options, LF line endings, trailing newline) so CI
// can regenerate and run `git diff --exit-code`.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { compile } from 'json-schema-to-typescript';

const here = dirname(fileURLToPath(import.meta.url));
const schemaPath = resolve(here, '../../schemas/promptcad.schema.json');
const outPath = resolve(here, '../src/api/generated/promptcad.d.ts');

const banner = [
  '/* eslint-disable */',
  '/**',
  ' * AUTO-GENERATED FILE: do not edit — generated from schemas/promptcad.schema.json.',
  ' * Regenerate with `npm run gen:types` (in web/) after running scripts/export_schema.py.',
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

const schema = stripNestedTitles(JSON.parse(await readFile(schemaPath, 'utf8')));

let ts = await compile(schema, 'PromptCADSchema', {
  bannerComment: banner,
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

ts = ts.replace(/\r\n?/g, '\n').replace(/\s+$/, '') + '\n';

await mkdir(dirname(outPath), { recursive: true });
await writeFile(outPath, ts, { encoding: 'utf8' });
console.log(`Wrote ${outPath}`);
