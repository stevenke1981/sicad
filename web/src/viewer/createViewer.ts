import {
  AmbientLight,
  Box3,
  Color,
  DirectionalLight,
  EdgesGeometry,
  GridHelper,
  HemisphereLight,
  LineBasicMaterial,
  LineSegments,
  Mesh,
  MeshStandardMaterial,
  PerspectiveCamera,
  Scene,
  Sphere,
  Vector3,
  WebGLRenderer,
} from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { parseStl } from './parseStl';
import { isWebGLAvailable } from './webgl';

export interface Viewer {
  /** Replace the displayed model with the given STL bytes (binary or ASCII). */
  loadStl(buffer: ArrayBuffer): void;
  /** Remove the current model, if any. */
  clear(): void;
  /** Release GPU resources, listeners and the render loop. Idempotent. */
  dispose(): void;
}

export class WebGLUnavailableError extends Error {
  constructor() {
    super('此環境不支援 WebGL，無法顯示 3D 模型。');
    this.name = 'WebGLUnavailableError';
  }
}

/**
 * Framework-free STL viewer (ADR 0004): orbit/pan/zoom with CAD Z-up orientation.
 * Renders on demand (only when the camera moves or the model/size changes).
 */
export function createViewer(canvas: HTMLCanvasElement): Viewer {
  if (!isWebGLAvailable(canvas.ownerDocument)) throw new WebGLUnavailableError();

  const renderer = new WebGLRenderer({ canvas, antialias: true, alpha: false });
  renderer.setPixelRatio(Math.min(globalThis.devicePixelRatio || 1, 2));

  const scene = new Scene();
  scene.background = new Color(0x090e1a);
  scene.add(new AmbientLight(0xffffff, 0.35));
  scene.add(new HemisphereLight(0xdfe8ff, 0x1a2033, 1.2));
  const key = new DirectionalLight(0xffffff, 1.6);
  key.position.set(1, -1.5, 2);
  scene.add(key);
  const fill = new DirectionalLight(0x9fb8ff, 0.6);
  fill.position.set(-1.5, 1, 0.5);
  scene.add(fill);

  const camera = new PerspectiveCamera(40, 1, 0.1, 10_000);
  camera.up.set(0, 0, 1);
  camera.position.set(150, -200, 150);

  const controls = new OrbitControls(camera, canvas);
  controls.enableDamping = true;
  controls.dampingFactor = 0.12;

  const material = new MeshStandardMaterial({ color: 0x9db6e8, metalness: 0.25, roughness: 0.55 });
  const edgeMaterial = new LineBasicMaterial({ color: 0x1b2742 });

  let mesh: Mesh | null = null;
  let edges: LineSegments | null = null;
  let grid: GridHelper | null = null;
  let frame = 0;
  let disposed = false;

  const render = () => {
    frame = 0;
    if (disposed) return;
    const moving = controls.update();
    renderer.render(scene, camera);
    if (moving) requestRender();
  };
  const requestRender = () => {
    if (!disposed && !frame) frame = requestAnimationFrame(render);
  };

  const resize = () => {
    const width = canvas.clientWidth || canvas.parentElement?.clientWidth || 1;
    const height = canvas.clientHeight || canvas.parentElement?.clientHeight || 1;
    renderer.setSize(width, height, false);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    requestRender();
  };

  controls.addEventListener('change', requestRender);
  const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(resize);
  observer?.observe(canvas);
  resize();

  const removeModel = () => {
    for (const object of [mesh, edges, grid]) {
      if (!object) continue;
      scene.remove(object);
      object.geometry.dispose();
    }
    grid?.dispose();
    mesh = edges = grid = null;
  };

  const frameModel = (sphere: Sphere, box: Box3) => {
    const radius = Math.max(sphere.radius, 1);
    const distance = radius / Math.sin((camera.fov * Math.PI) / 360);
    const direction = new Vector3(1, -1.3, 0.9).normalize();
    camera.position.copy(sphere.center).addScaledVector(direction, distance * 1.15);
    camera.near = Math.max(distance / 1000, 0.01);
    camera.far = distance * 100;
    camera.updateProjectionMatrix();
    controls.target.copy(sphere.center);
    controls.update();

    const size = Math.ceil((radius * 4) / 10) * 10;
    grid = new GridHelper(size, 20, 0x3a4a6a, 0x1f2a40);
    grid.rotation.x = Math.PI / 2; // GridHelper lies in XZ; CAD floor is XY.
    grid.position.set(sphere.center.x, sphere.center.y, box.min.z);
    scene.add(grid);
  };

  return {
    loadStl(buffer: ArrayBuffer) {
      if (disposed) throw new Error('Viewer has been disposed.');
      const geometry = parseStl(buffer);
      removeModel();
      mesh = new Mesh(geometry, material);
      edges = new LineSegments(new EdgesGeometry(geometry, 30), edgeMaterial);
      scene.add(mesh, edges);
      frameModel(geometry.boundingSphere ?? new Sphere(), geometry.boundingBox ?? new Box3());
      requestRender();
    },
    clear() {
      removeModel();
      requestRender();
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      if (frame) cancelAnimationFrame(frame);
      observer?.disconnect();
      controls.removeEventListener('change', requestRender);
      controls.dispose();
      removeModel();
      material.dispose();
      edgeMaterial.dispose();
      renderer.dispose();
    },
  };
}
