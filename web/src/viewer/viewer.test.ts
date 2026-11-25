import { describe, expect, it } from 'vitest';
import tetrahedronStl from './__fixtures__/tetrahedron.stl?raw';
import { createViewer, isWebGLAvailable, parseStl, WebGLUnavailableError } from './index';

function toArrayBuffer(text: string): ArrayBuffer {
  const bytes = new TextEncoder().encode(text);
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

describe('parseStl', () => {
  it('parses an ASCII STL fixture into non-empty geometry', () => {
    const geometry = parseStl(toArrayBuffer(tetrahedronStl));
    const position = geometry.getAttribute('position');

    expect(position.count).toBe(12); // 4 facets x 3 vertices
    expect(geometry.getAttribute('normal').count).toBe(12);
    const box = geometry.boundingBox!;
    expect(box.min.toArray()).toEqual([0, 0, 0]);
    expect(box.max.toArray()).toEqual([10, 10, 10]);
    expect(geometry.boundingSphere!.radius).toBeGreaterThan(0);
  });

  it('rejects an STL with no triangles', () => {
    const empty = new TextEncoder().encode('solid empty\nendsolid empty\n');
    expect(() => parseStl(empty.buffer as ArrayBuffer)).toThrow();
  });
});

describe('createViewer', () => {
  it('reports WebGL as unavailable under jsdom and refuses to start', () => {
    expect(isWebGLAvailable()).toBe(false);
    expect(() => createViewer(document.createElement('canvas'))).toThrow(WebGLUnavailableError);
  });
});
