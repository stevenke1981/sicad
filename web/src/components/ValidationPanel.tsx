import type { ValidationIssue, ValidationReport, ValidationSeverity } from '../api/types';

/** 1.0 issues plus the optional fields a 2.0 (Rust core) issue carries. */
export type PanelIssue = ValidationIssue & {
  target?: string | null;
  related?: string[];
  owner?: string | null;
  layer?: string;
};
export type PanelReport = Omit<ValidationReport, 'issues'> & { issues?: PanelIssue[] };

const LAYER_LABEL: Record<string, string> = { analytic: '解析層', kernel: '核心層' };

interface Props {
  report: PanelReport | null;
  /** Where the report came from: the job manifest or an explicit "validate" call. */
  source: 'job' | 'validate';
  /** Extra renderer/job warnings from the manifest (strings, not issues). */
  warnings: string[];
  /** Warnings about the submitted input itself (e.g. numbers the transport altered). */
  inputWarnings?: string[];
}

const SEVERITY_LABEL: Record<ValidationSeverity, string> = {
  error: '錯誤',
  warning: '警告',
  info: '資訊',
};

const SEVERITY_ORDER: ValidationSeverity[] = ['error', 'warning', 'info'];

function IssueRefs({ issue }: { issue: PanelIssue }) {
  const parts = [
    issue.target ? `目標：${issue.target}` : '',
    issue.related?.length ? `相關：${issue.related.join('、')}` : '',
    issue.owner ? `負責：${issue.owner}` : '',
    issue.layer ? `層級：${LAYER_LABEL[issue.layer] ?? issue.layer}` : '',
  ].filter(Boolean);
  return parts.length ? <small className="issue-ref">{parts.join(' · ')}</small> : null;
}

export function ValidationPanel({ report, source, warnings, inputWarnings = [] }: Props) {
  if (!report && !warnings.length && !inputWarnings.length) return null;
  const issues = report?.issues ?? [];
  const counts = SEVERITY_ORDER.map((severity) => ({
    severity,
    count: issues.filter((issue) => issue.severity === severity).length,
  }));
  const sorted = [...issues].sort(
    (a, b) => SEVERITY_ORDER.indexOf(a.severity) - SEVERITY_ORDER.indexOf(b.severity),
  );

  return (
    <section className="validation" aria-label="驗證報告">
      <div className="validation-head">
        <strong>{source === 'validate' ? 'JSON 驗證結果' : '驗證報告'}</strong>
        {report && (
          <span className={`pill ${report.valid ? 'ok' : 'fail'}`}>
            {report.valid ? '有效' : '無效'}
          </span>
        )}
        {report?.review_required && <span className="pill warn">需人工覆核</span>}
        {counts.map(({ severity, count }) => (
          <span key={severity} className={`count ${severity}`}>
            {SEVERITY_LABEL[severity]} {count}
          </span>
        ))}
      </div>
      {inputWarnings.length > 0 && (
        <ul className="issues" aria-label="輸入警告">
          {[...new Set(inputWarnings)].map((text) => (
            <li key={`i-${text}`} className="issue warning">
              <span className="sev">輸入</span>
              <span className="msg">{text}</span>
            </li>
          ))}
        </ul>
      )}
      {warnings.length > 0 && (
        <ul className="issues">
          {[...new Set(warnings)].map((text) => (
            <li key={`w-${text}`} className="issue warning">
              <span className="sev">輸出</span>
              <span className="msg">{text}</span>
            </li>
          ))}
        </ul>
      )}
      {sorted.length > 0 ? (
        <ul className="issues">
          {sorted.map((issue, index) => (
            <li key={`${issue.code}-${index}`} className={`issue ${issue.severity}`}>
              <span className="sev">{SEVERITY_LABEL[issue.severity]}</span>
              <span className="msg">
                {issue.message}
                <small>
                  {issue.code}
                  {issue.feature_index !== null && issue.feature_index !== undefined
                    ? ` · 特徵 #${issue.feature_index}`
                    : ''}
                </small>
                <IssueRefs issue={issue} />
              </span>
            </li>
          ))}
        </ul>
      ) : (
        report && <p className="muted">沒有驗證問題。</p>
      )}
    </section>
  );
}
