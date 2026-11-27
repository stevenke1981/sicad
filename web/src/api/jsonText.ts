// Helpers that work on JSON *text* instead of parsed values, so documents typed in the editor
// reach the core with the exact digits the user wrote. `JSON.parse` turns integers above 2^53
// into the nearest double (a `revision` of 2^64-1 becomes 18446744073709552000), writes
// integers ≥ 1e21 back in exponent form (which the core rejects for integer fields), drops
// the sign of `-0` and turns out-of-range values into `null`.
//
// The scanner below assumes text that `JSON.parse` already accepted; it does not validate.

/** A number token whose `JSON.parse` → `JSON.stringify` round trip changes what the core sees. */
export interface LossyNumber {
  /** JSON pointer of the value (`/revision`, `/components/3/shape/length`). */
  pointer: string;
  /** The token as written. */
  token: string;
  /** What `JSON.stringify(JSON.parse(token))` sends instead. */
  sent: string;
}

const WS = new Set([' ', '\t', '\n', '\r']);

function escapePointer(segment: string): string {
  return segment.replace(/~/g, '~0').replace(/\//g, '~1');
}

const INTEGER = /^-?(0|[1-9]\d*)$/;

/** Why a number token is lossy, or `null` when it survives the round trip. */
function lossy(token: string): string | null {
  const value = Number(token);
  const sent = JSON.stringify(value);
  if (INTEGER.test(token)) {
    // The core reads an integer token as an exact integer.
    if (!INTEGER.test(sent) || BigInt(token) !== BigInt(sent)) return sent;
  }
  // `-0`, `-0.0`, `-0e5`: the sign is lost.
  if (token.startsWith('-') && value === 0) return sent;
  // `1e400`: parses to Infinity, which JSON.stringify writes as `null`.
  if (!Number.isFinite(value)) return sent;
  return null;
}

interface Member {
  key: string;
  /** Start of the key token. */
  start: number;
  /** End (exclusive) of the value. */
  end: number;
}

/** Minimal recursive-descent walker over valid JSON text. */
class Walker {
  i = 0;
  readonly numbers: LossyNumber[] = [];
  readonly topMembers: Member[] = [];

  constructor(private readonly text: string) {}

  private ws() {
    while (this.i < this.text.length && WS.has(this.text[this.i]!)) this.i += 1;
  }

  private string(): string {
    const start = this.i;
    this.i += 1; // opening quote
    while (this.text[this.i] !== '"') {
      this.i += this.text[this.i] === '\\' ? 2 : 1;
    }
    this.i += 1;
    return JSON.parse(this.text.slice(start, this.i)) as string;
  }

  value(pointer: string, depth: number) {
    this.ws();
    const c = this.text[this.i];
    if (c === '{') {
      this.i += 1;
      this.ws();
      if (this.text[this.i] === '}') {
        this.i += 1;
        return;
      }
      for (;;) {
        this.ws();
        const start = this.i;
        const key = this.string();
        this.ws();
        this.i += 1; // colon
        this.value(`${pointer}/${escapePointer(key)}`, depth + 1);
        if (depth === 0) this.topMembers.push({ key, start, end: this.i });
        this.ws();
        if (this.text[this.i] === ',') {
          this.i += 1;
          continue;
        }
        this.i += 1; // closing brace
        return;
      }
    }
    if (c === '[') {
      this.i += 1;
      this.ws();
      if (this.text[this.i] === ']') {
        this.i += 1;
        return;
      }
      for (let index = 0; ; index += 1) {
        this.value(`${pointer}/${index}`, depth + 1);
        this.ws();
        if (this.text[this.i] === ',') {
          this.i += 1;
          continue;
        }
        this.i += 1; // closing bracket
        return;
      }
    }
    if (c === '"') {
      this.string();
      return;
    }
    const start = this.i;
    while (this.i < this.text.length && /[-+.eE\d]/.test(this.text[this.i]!)) this.i += 1;
    if (this.i === start) {
      // true / false / null
      while (this.i < this.text.length && /[a-z]/.test(this.text[this.i]!)) this.i += 1;
      return;
    }
    const token = this.text.slice(start, this.i);
    const sent = lossy(token);
    if (sent !== null) this.numbers.push({ pointer, token, sent });
  }
}

function walk(text: string): Walker {
  const walker = new Walker(text);
  walker.value('', 0);
  return walker;
}

/**
 * Number tokens in `text` (valid JSON) that `JSON.parse` + `JSON.stringify` would alter:
 * integers outside ±(2^53 − 1), negative zeros and out-of-range values (`1e400` → `null`).
 * Empty when the round trip is exact.
 */
export function findLossyNumbers(text: string): LossyNumber[] {
  return walk(text).numbers;
}

/**
 * `text` (a valid JSON object) without the top-level members named `key` (every duplicate).
 * The other members are copied verbatim, including their numbers and inner whitespace.
 */
export function removeTopLevelKey(text: string, key: string): string {
  const members = walk(text).topMembers;
  if (!members.some((m) => m.key === key)) return text;
  return `{${members
    .filter((m) => m.key !== key)
    .map((m) => text.slice(m.start, m.end))
    .join(',')}}`;
}
