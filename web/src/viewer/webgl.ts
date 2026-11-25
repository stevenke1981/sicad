// Kept free of three.js imports so UI code can probe WebGL without loading the viewer chunk.

/** True when a WebGL2 or WebGL1 context can be created in this environment. */
export function isWebGLAvailable(doc: Document = document): boolean {
  try {
    const canvas = doc.createElement('canvas');
    const gl = canvas.getContext('webgl2') ?? canvas.getContext('webgl');
    if (!gl) return false;
    // Release the probe context right away instead of waiting for GC.
    (gl as WebGLRenderingContext).getExtension('WEBGL_lose_context')?.loseContext();
    return true;
  } catch {
    return false;
  }
}
