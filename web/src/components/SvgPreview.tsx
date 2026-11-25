import { useEffect, useState } from 'react';
import type { Artifact } from '../api/types';

interface Props {
  artifact: Artifact | null;
  fetchArtifact: (url: string) => Promise<Blob>;
}

/**
 * Shows the SVG preview fetched with auth headers. It is rendered through <img> from a
 * blob: URL (never innerHTML), so scripts or external references inside the SVG cannot run.
 *
 * `model.svg` (the CadQuery projection) is drawn with dark strokes on a transparent
 * background, so it gets an off-white "paper" backdrop; `preview.svg` paints its own dark
 * background and keeps the dark wrapper.
 */
export function SvgPreview({ artifact, fetchArtifact }: Props) {
  const [src, setSrc] = useState<string | null>(null);
  const [message, setMessage] = useState('生成後會顯示工程預覽');

  useEffect(() => {
    setSrc(null);
    if (!artifact) {
      setMessage('生成後會顯示工程預覽');
      return;
    }
    let cancelled = false;
    let objectUrl: string | null = null;
    setMessage('正在載入預覽…');
    fetchArtifact(artifact.url)
      .then((blob) => {
        if (cancelled) return;
        objectUrl = URL.createObjectURL(new Blob([blob], { type: 'image/svg+xml' }));
        setSrc(objectUrl);
      })
      .catch((error: unknown) => {
        if (!cancelled) setMessage(`預覽載入失敗：${errorText(error)}`);
      });
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [artifact, fetchArtifact]);

  const kind = artifact ? previewKind(artifact.filename) : undefined;
  const className = kind === 'model' ? 'preview-wrap preview-wrap--paper' : 'preview-wrap';

  return (
    <div className={className} data-kind={kind}>
      {src ? (
        <img src={src} alt={`${artifact?.filename ?? 'CAD'} 預覽`} />
      ) : (
        <div className="preview-empty">{message}</div>
      )}
    </div>
  );
}

export function previewKind(filename: string): 'model' | 'preview' {
  return filename === 'model.svg' ? 'model' : 'preview';
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
