import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { Artifact } from '../api/types';
import { SvgPreview } from './SvgPreview';

function artifact(filename: string): Artifact {
  return { filename, media_type: 'image/svg+xml', size: 100, url: `/api/v1/jobs/j1/files/${filename}` };
}

const okFetch = () => vi.fn(async () => new Blob(['<svg/>']));

function wrapper(container: HTMLElement): HTMLElement {
  const element = container.querySelector('.preview-wrap');
  if (!(element instanceof HTMLElement)) throw new Error('preview wrapper not rendered');
  return element;
}

describe('SvgPreview', () => {
  it('puts model.svg on a paper backdrop', async () => {
    const fetchArtifact = okFetch();
    const { container } = render(<SvgPreview artifact={artifact('model.svg')} fetchArtifact={fetchArtifact} />);

    expect(await screen.findByRole('img', { name: /model\.svg/ })).toBeInTheDocument();
    expect(fetchArtifact).toHaveBeenCalledWith('/api/v1/jobs/j1/files/model.svg');
    const wrap = wrapper(container);
    expect(wrap).toHaveClass('preview-wrap', 'preview-wrap--paper');
    expect(wrap).toHaveAttribute('data-kind', 'model');
  });

  it('keeps the dark wrapper for preview.svg', async () => {
    const { container } = render(<SvgPreview artifact={artifact('preview.svg')} fetchArtifact={okFetch()} />);

    expect(await screen.findByRole('img', { name: /preview\.svg/ })).toBeInTheDocument();
    const wrap = wrapper(container);
    expect(wrap).toHaveClass('preview-wrap');
    expect(wrap).not.toHaveClass('preview-wrap--paper');
    expect(wrap).toHaveAttribute('data-kind', 'preview');
  });

  it('shows the placeholder without an artifact', () => {
    const fetchArtifact = okFetch();
    const { container } = render(<SvgPreview artifact={null} fetchArtifact={fetchArtifact} />);

    expect(screen.getByText('生成後會顯示工程預覽')).toBeInTheDocument();
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
    expect(fetchArtifact).not.toHaveBeenCalled();
    const wrap = wrapper(container);
    expect(wrap).not.toHaveClass('preview-wrap--paper');
    expect(wrap).not.toHaveAttribute('data-kind');
  });

  it('reports a failed fetch', async () => {
    const fetchArtifact = vi.fn(async () => {
      throw new Error('HTTP 404');
    });
    render(<SvgPreview artifact={artifact('model.svg')} fetchArtifact={fetchArtifact} />);

    expect(await screen.findByText('預覽載入失敗：HTTP 404')).toBeInTheDocument();
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
  });
});
