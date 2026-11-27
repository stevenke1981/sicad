import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { enclosureDescription, plateDescription, repeatedDescription } from '../test/fixtures';
import { ComponentTree } from './ComponentTree';

describe('ComponentTree', () => {
  it('renders the assembly/part tree of a real describe result', () => {
    render(<ComponentTree description={enclosureDescription()} />);
    const tree = screen.getByRole('list', { name: '組件與零件' });
    expect(within(tree).getByText('電子外殼')).toBeInTheDocument();
    const body = screen.getByRole('list', { name: '外殼本體 的本體操作' });
    expect(within(body).getByText('v_body')).toBeInTheDocument();
    expect(within(body).getByText('enclosure')).toBeInTheDocument();
    expect(within(body).getByText('volume')).toBeInTheDocument();
    expect(screen.getByRole('list', { name: '上蓋 的本體操作' })).toBeInTheDocument();
  });

  it('expands a component to show fields (value, bound + expr) and ports with types', async () => {
    const user = userEvent.setup();
    render(<ComponentTree description={plateDescription()} />);
    const body = screen.getByRole('list', { name: '固定板 的本體操作' });
    const details = body.querySelector('details');
    expect(details).not.toBeNull();
    expect(details).not.toHaveAttribute('open');

    await user.click(within(body).getByText('v_plate'));
    expect(details).toHaveAttribute('open');

    const fields = within(body).getByRole('table', { name: 'v_plate 的欄位' });
    const lengthRow = within(fields).getByRole('row', { name: /length/ });
    expect(within(lengthRow).getByText('120')).toBeInTheDocument();
    expect(within(lengthRow).getByText('len')).toBeInTheDocument();
    const ports = within(details!).getByText('連接點：').parentElement!;
    expect(within(ports).getByText('origin')).toBeInTheDocument();
    expect(ports).toHaveTextContent('point');
  });

  it('lists constraints with id, kind and both ports', () => {
    render(<ComponentTree description={enclosureDescription()} />);
    const list = screen.getByRole('list', { name: '約束' });
    expect(within(list).getAllByRole('listitem')).toHaveLength(5);
    const row = within(list).getByText('k_lid_axis').closest('li')!;
    expect(row).toHaveTextContent('coaxial');
    expect(row).toHaveTextContent('v_lid.axis_z ↔ v_body.axis_z');
    expect(within(list).getByText('k_root').closest('li')).toHaveTextContent('v_body.origin');
  });
  it('renders repeated nodes (real describe output) as back-references without duplicate keys', () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    try {
      render(<ComponentTree description={repeatedDescription()} />);
      const tree = screen.getByRole('list', { name: '組件與零件' });
      // p_body: listed twice in a_main → first expanded with its body, second a reference
      const partNodes = within(tree)
        .getAllByText('p_body')
        .filter((el) => el.classList.contains('muted'));
      expect(partNodes).toHaveLength(2);
      expect(within(tree).getAllByRole('list', { name: '外殼本體 的本體操作' })).toHaveLength(1);
      const listedAbove = within(tree).getByText('已於上方列出');
      expect(listedAbove.closest('li')).toHaveTextContent('p_body');
      // a_sub: child of a_main and a_other → expanded once, referenced once
      const expandedAbove = within(tree).getByText('已於上方展開');
      expect(expandedAbove.closest('li')).toHaveTextContent('上蓋組');
      expect(expandedAbove.closest('li')).toHaveTextContent('a_sub');
      expect(expandedAbove.closest('li')!.querySelector('ul')).toBeNull();
      expect(within(tree).getAllByRole('list', { name: '上蓋 的本體操作' })).toHaveLength(1);
      // the arrow is decorative
      expect(expandedAbove.closest('li')!.querySelector('[aria-hidden="true"]')).toHaveTextContent(
        '↩',
      );
      const keyWarnings = consoleError.mock.calls.filter((args) =>
        String(args[0]).includes('same key'),
      );
      expect(keyWarnings).toEqual([]);
    } finally {
      consoleError.mockRestore();
    }
  });

  it('uses index-qualified keys for duplicate ids in hand-made descriptions', () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    try {
      const base = enclosureDescription();
      const node = base.tree[0]!;
      const description = {
        ...base,
        tree: [{ ...node, children: [node.children[0]!, node.children[0]!] }, node],
        constraints: [base.constraints[0]!, base.constraints[0]!],
      };
      render(<ComponentTree description={description} />);
      expect(screen.getAllByRole('list', { name: '外殼本體 的本體操作' }).length).toBeGreaterThan(1);
      expect(within(screen.getByRole('list', { name: '約束' })).getAllByRole('listitem')).toHaveLength(2);
      expect(
        consoleError.mock.calls.filter((args) => String(args[0]).includes('same key')),
      ).toEqual([]);
    } finally {
      consoleError.mockRestore();
    }
  });
});
