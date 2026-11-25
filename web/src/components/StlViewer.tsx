import { useEffect, useRef, useState } from 'react';
import type { Artifact } from '../api/types';
import type { Viewer } from '../viewer/createViewer';
import { isWebGLAvailable } from '../viewer/webgl';

interface Props {
  artifact: Artifact | null;
  fetchArtifact: (url: string) => Promise<Blob>;
  /** Shown when there is no STL artifact (e.g. source_only jobs). */
  emptyMessage: string;
}

/** React wrapper around the framework-free viewer; three.js is loaded lazily. */
export function StlViewer({ artifact, fetchArtifact, emptyMessage }: Props) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const viewerRef = useRef<Viewer | null>(null);
  const [webgl] = useState(() => isWebGLAvailable());
  const [ready, setReady] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!webgl || !canvas) return;
    let cancelled = false;
    let viewer: Viewer | null = null;
    import('../viewer/createViewer')
      .then(({ createViewer }) => {
        if (cancelled) return;
        viewer = createViewer(canvas);
        viewerRef.current = viewer;
        setReady(true);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(`3D 檢視器初始化失敗：${errorText(err)}`);
      });
    return () => {
      cancelled = true;
      viewer?.dispose();
      viewerRef.current = null;
      setReady(false);
    };
  }, [webgl]);

  useEffect(() => {
    const viewer = viewerRef.current;
    if (!ready || !viewer) return;
    setError(null);
    if (!artifact) {
      viewer.clear();
      return;
    }
    let cancelled = false;
    setLoading(true);
    fetchArtifact(artifact.url)
      .then((blob) => blob.arrayBuffer())
      .then((buffer) => {
        if (!cancelled) viewer.loadStl(buffer);
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          viewer.clear();
          setError(`STL 載入失敗：${errorText(err)}`);
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [ready, artifact, fetchArtifact]);

  let overlay: string | null = null;
  if (!webgl) overlay = '此環境不支援 WebGL，無法顯示 3D 模型。請改用 2D 預覽或下載 STL。';
  else if (error) overlay = error;
  else if (!artifact) overlay = emptyMessage;
  else if (loading || !ready) overlay = '正在載入 STL…';

  return (
    <div className="viewer-wrap" data-testid="stl-viewer">
      <canvas ref={canvasRef} hidden={!webgl} aria-label="3D 模型檢視器" />
      {overlay && (
        <div className={`viewer-overlay${error || !webgl ? ' error' : ''}`} role="status">
          {overlay}
        </div>
      )}
      {webgl && ready && artifact && !error && !loading && (
        <div className="viewer-hint">拖曳旋轉 · 右鍵平移 · 滾輪縮放</div>
      )}
    </div>
  );
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
