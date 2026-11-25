import type { Artifact } from '../api/types';
import { formatBytes } from '../download';

interface Props {
  jobId: string;
  artifacts: Artifact[];
  bundleUrl: string;
  onDownload: (url: string, filename: string) => void;
}

export function ArtifactList({ jobId, artifacts, bundleUrl, onDownload }: Props) {
  return (
    <div className="downloads" aria-label="輸出檔案">
      {artifacts.map((artifact) => (
        <button
          key={artifact.filename}
          type="button"
          className="download"
          title={`${artifact.media_type} · ${formatBytes(artifact.size)}`}
          onClick={() => onDownload(artifact.url, artifact.filename)}
        >
          {artifact.filename}
          <small>{formatBytes(artifact.size)}</small>
        </button>
      ))}
      <button
        type="button"
        className="download bundle"
        onClick={() => onDownload(bundleUrl, `promptcad-${jobId}.zip`)}
      >
        全部打包 ZIP
      </button>
    </div>
  );
}
