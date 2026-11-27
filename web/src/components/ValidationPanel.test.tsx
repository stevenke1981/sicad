import { render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ValidationPanel, type PanelReport } from './ValidationPanel';
import { issueCodeLabel, ISSUE_CODE_LABELS } from './issueCodeLabels';

const report: PanelReport = {
  valid: false,
  review_required: true,
  issues: [
    {
      severity: 'error',
      code: 'overlapping_holes',
      message: '孔 1 與孔 2 發生重疊。',
      feature_index: 0,
    },
    {
      severity: 'warning',
      code: 'some_unknown_code',
      message: '未知的驗證訊息。',
      feature_index: null,
    },
  ],
} as PanelReport;

describe('ValidationPanel issue labels', () => {
  it('shows the zh-TW label before the original message for a known code', () => {
    render(<ValidationPanel report={report} source="validate" warnings={[]} />);
    const list = screen.getByRole('list');
    const items = within(list).getAllByRole('listitem');
    const known = items[0]!;
    expect(known.querySelector('.issue-label')).toHaveTextContent('孔位重疊');
    expect(known.querySelector('.msg')).toHaveTextContent('孔 1 與孔 2 發生重疊。');
  });

  it('shows only the message for an unknown code', () => {
    render(<ValidationPanel report={report} source="validate" warnings={[]} />);
    const items = within(screen.getByRole('list')).getAllByRole('listitem');
    const unknown = items[1]!;
    expect(unknown.querySelector('.issue-label')).toBeNull();
    expect(unknown.querySelector('.msg')).toHaveTextContent('未知的驗證訊息。');
  });

  it('returns undefined for prototype keys such as "constructor"', () => {
    expect(issueCodeLabel('constructor')).toBeUndefined();
    expect(issueCodeLabel('__proto__')).toBeUndefined();
    expect(issueCodeLabel('overlapping_holes')).toBe('孔位重疊');
    expect(Object.keys(ISSUE_CODE_LABELS).length).toBeGreaterThanOrEqual(60);
  });
});
