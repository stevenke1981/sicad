import type { JobListItem } from '../api/types';

interface Props {
  jobs: JobListItem[] | null;
  error: string | null;
  activeJobId: string | null;
  onOpen: (jobId: string) => void;
  onRefresh: () => void;
}

export function JobHistory({ jobs, error, activeJobId, onOpen, onRefresh }: Props) {
  return (
    <section className="history panel" aria-label="最近工作">
      <div className="result-head">
        <div>
          <div className="eyebrow">RECENT JOBS</div>
          <h2>最近工作</h2>
        </div>
        <button type="button" className="secondary" onClick={onRefresh}>
          重新整理
        </button>
      </div>
      {error ? (
        <p className="status error">{error}</p>
      ) : jobs === null ? (
        <p className="muted">載入中…</p>
      ) : jobs.length === 0 ? (
        <p className="muted">尚無工作。</p>
      ) : (
        <div className="jobs">
          {jobs.map((job) => (
            <button
              key={job.job_id}
              type="button"
              className={`job${job.job_id === activeJobId ? ' active' : ''}`}
              onClick={() => onOpen(job.job_id)}
            >
              <strong>{job.name}</strong>
              <small>{job.prompt}</small>
              <small>
                {job.status} · {job.renderer_used}
              </small>
              <small>{formatTime(job.created_at)}</small>
            </button>
          ))}
        </div>
      )}
    </section>
  );
}

function formatTime(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString('zh-TW', { hour12: false });
}
