import type { BufferGeometry } from 'three';
import { STLLoader } from 'three/examples/jsm/loaders/STLLoader.js';

/**
 * Parse binary or ASCII STL bytes into a BufferGeometry with normals and bounds computed.
 * Coordinates are kept as exported (CAD origin, millimetres, Z up).
 * Pure (no WebGL), so it is unit-testable under jsdom/node.
 */
export function parseStl(buffer: ArrayBuffer): BufferGeometry {
  const geometry = new STLLoader().parse(buffer);
  const position = geometry.getAttribute('position');
  if (!position || position.count === 0) {
    geometry.dispose();
    throw new Error('STL 不含任何三角形。');
  }
  if (!geometry.getAttribute('normal')) geometry.computeVertexNormals();
  geometry.computeBoundingBox();
  geometry.computeBoundingSphere();
  return geometry;
}
