import { describe, expect, it } from 'vitest';
import { findLossyNumbers, removeTopLevelKey } from './jsonText';

describe('findLossyNumbers', () => {
  it('flags integers beyond 2^53, integers ≥ 1e21, negative zeros and overflows, with pointers', () => {
    const text = `{
      "revision": 18446744073709551615,
      "safe": 9007199254740991,
      "unsafe": 9007199254740993,
      "huge": 1000000000000000000000,
      "list": [1, -0, 2.5, -0.0, 1e400, 3e2],
      "a/b~c": { "x": -9007199254740993 },
      "str": "18446744073709551615 -0",
      "flags": [true, false, null]
    }`;
    expect(findLossyNumbers(text)).toEqual([
      { pointer: '/revision', token: '18446744073709551615', sent: '18446744073709552000' },
      { pointer: '/unsafe', token: '9007199254740993', sent: '9007199254740992' },
      { pointer: '/huge', token: '1000000000000000000000', sent: '1e+21' },
      { pointer: '/list/1', token: '-0', sent: '0' },
      { pointer: '/list/3', token: '-0.0', sent: '0' },
      { pointer: '/list/4', token: '1e400', sent: 'null' },
      { pointer: '/a~1b~0c/x', token: '-9007199254740993', sent: '-9007199254740992' },
    ]);
  });

  it('is empty for documents whose numbers survive JSON.parse / JSON.stringify', () => {
    expect(findLossyNumbers('{"revision": 4, "x": [0, 1.5, -2, 1e-7, 120.0], "s": "-0"}')).toEqual(
      [],
    );
    expect(findLossyNumbers('{}')).toEqual([]);
    expect(findLossyNumbers('{"a": [], "b": {}, "c": "\\"quoted\\" \\\\"}')).toEqual([]);
  });
});

describe('removeTopLevelKey', () => {
  it('removes every top-level occurrence and keeps the rest verbatim', () => {
    const text =
      '{ "schema_version": "2.0", "expected_resolved": {"revision": 1},\n' +
      '  "revision": 18446744073709551615, "x": -0,\n' +
      '  "nested": {"expected_resolved": 1}, "expected_resolved": [1] }';
    const out = removeTopLevelKey(text, 'expected_resolved');
    expect(out).toBe(
      '{"schema_version": "2.0","revision": 18446744073709551615,"x": -0,"nested": {"expected_resolved": 1}}',
    );
    expect(JSON.parse(out)).toEqual({
      schema_version: '2.0',
      revision: Number('18446744073709551615'),
      x: -0,
      nested: { expected_resolved: 1 },
    });
  });

  it('matches escaped keys and leaves documents without the key untouched', () => {
    expect(removeTopLevelKey('{"expected\\u005fresolved": 1, "a": 2}', 'expected_resolved')).toBe(
      '{"a": 2}',
    );
    expect(removeTopLevelKey('{"expected_resolved": 1}', 'expected_resolved')).toBe('{}');
    const plain = ' {"a": 18446744073709551615} ';
    expect(removeTopLevelKey(plain, 'expected_resolved')).toBe(plain);
  });
});
