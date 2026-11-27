import type { ResultOf } from '../api/coreClient';

export type Description = ResultOf<'describe'>;

export type ResolvedPorts = ResultOf<'validate'>['resolved']['ports'];
type ResolvedPort = ResolvedPorts[string];

interface Props {
  description: Description;
  /** Optional world geometry of the ports, keyed `<component id>.<port name>` (from `validate`). */
  resolvedPorts?: ResolvedPorts;
}

type PartView = Description['parts'][number];
type ComponentView = Description['components'][number];
type TreeNode = Description['tree'][number];

const OP_LABEL = { add: '加', cut: '減', intersect: '交' } as const;

function formatValue(value: number | null): string {
  return value === null ? '—' : String(value);
}

/** At most 4 decimals, trailing zeros trimmed, -0 shown as 0. */
function formatNumber(value: number): string {
  const rounded = Number(value.toFixed(4));
  return String(rounded === 0 ? 0 : rounded);
}

function formatVec(values: number[]): string {
  return `(${values.map(formatNumber).join(', ')})`;
}

function describePort(port: ResolvedPort): string {
  switch (port.type) {
    case 'point':
      return formatVec(port.position);
    case 'axis':
      return `${formatVec(port.position)} 方向 ${port.direction}`;
    case 'face':
      return `${formatVec(port.origin)} 法向 ${port.normal}`;
    case 'region':
      return `${formatVec(port.min)}–${formatVec(port.max)}`;
    default:
      return '';
  }
}

function ComponentDetails({
  component,
  resolvedPorts,
}: {
  component: ComponentView;
  resolvedPorts?: ResolvedPorts;
}) {
  return (
    <details className="ct-component">
      <summary>
        <span className="mono">{component.id}</span>{' '}
        <span className="ct-tag">{component.kind}</span>
        {component.role && <span className="ct-tag">{component.role}</span>}
        <span className="ct-tag">{component.shape}</span>
        {component.label && <span className="ct-label">{component.label}</span>}
      </summary>
      <div className="ct-body">
        {component.fields.length > 0 && (
          <table className="ct-table" aria-label={`${component.id} 的欄位`}>
            <thead>
              <tr>
                <th scope="col">欄位</th>
                <th scope="col">值</th>
                <th scope="col">綁定</th>
              </tr>
            </thead>
            <tbody>
              {component.fields.map((field) => (
                <tr key={field.name}>
                  <th scope="row">{field.name}</th>
                  <td>{formatValue(field.value)}</td>
                  <td className="mono">{field.bound ? (field.expr ?? '（綁定）') : ''}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {component.ports.length > 0 ? (
          <p className="ct-ports">
            <span className="muted">連接點：</span>
            {component.ports.map((port) => {
              const resolved = resolvedPorts?.[`${component.id}.${port.name}`];
              return (
                <span key={port.name} className="ct-port">
                  <span className="mono">{port.name}</span>
                  <small>{port.type}</small>
                  {resolved && <small className="ct-coord mono">{describePort(resolved)}</small>}
                </span>
              );
            })}
          </p>
        ) : (
          <p className="muted">沒有連接點。</p>
        )}
      </div>
    </details>
  );
}

function PartBody({
  part,
  components,
  resolvedPorts,
}: {
  part: PartView;
  components: Map<string, ComponentView>;
  resolvedPorts?: ResolvedPorts;
}) {
  if (part.body.length === 0) return <p className="muted">此零件沒有本體操作。</p>;
  return (
    <ul className="ct-list ct-body-ops" aria-label={`${part.name} 的本體操作`}>
      {part.body.map((op, index) => {
        const component = components.get(op.component);
        return (
          <li key={`${op.component}-${index}`}>
            <span className={`ct-op ${op.op}`}>{OP_LABEL[op.op]}</span>
            {component ? (
              <ComponentDetails component={component} resolvedPorts={resolvedPorts} />
            ) : (
              <span className="mono">{op.component}（找不到元件）</span>
            )}
          </li>
        );
      })}
    </ul>
  );
}

function Node({
  node,
  parts,
  components,
  resolvedPorts,
}: {
  node: TreeNode;
  parts: Map<string, PartView>;
  components: Map<string, ComponentView>;
  resolvedPorts?: ResolvedPorts;
}) {
  if (node.node === 'missing') {
    return (
      <li>
        <span className="mono">{node.id}</span> <span className="ct-tag warn">找不到</span>
      </li>
    );
  }
  const kindLabel = node.node === 'assembly' ? '組件' : '零件';
  if (node.repeated) {
    // A back-reference to the first occurrence (the document's assembly graph is not a tree):
    // listed once, never expanded again.
    const where = node.node === 'assembly' ? '已於上方展開' : '已於上方列出';
    return (
      <li className="ct-repeated">
        <div className="ct-node">
          <span className="ct-tag">{kindLabel}</span>
          <strong>{node.name ?? node.id}</strong>
          <span className="mono muted">{node.id}</span>
          <span className="muted">
            <span aria-hidden="true">↩ </span>
            {where}
          </span>
        </div>
      </li>
    );
  }
  const part = node.node === 'part' ? parts.get(node.id) : undefined;
  return (
    <li>
      <div className="ct-node">
        <span className="ct-tag">{kindLabel}</span>
        <strong>{node.name ?? node.id}</strong>
        <span className="mono muted">{node.id}</span>
        {part && (
          <span className="muted">
            · {part.source === 'purchased' ? '外購' : '自製'} · 數量 {part.quantity}
            {part.material ? ` · ${part.material}` : ''}
          </span>
        )}
      </div>
      {node.children.length > 0 && (
        <ul className="ct-list">
          {node.children.map((child, index) => (
            // Index-qualified: a child id can occur more than once (`repeated` nodes).
            <Node
              key={`${index}-${child.id}`}
              node={child}
              parts={parts}
              components={components}
              resolvedPorts={resolvedPorts}
            />
          ))}
        </ul>
      )}
      {part && <PartBody part={part} components={components} resolvedPorts={resolvedPorts} />}
    </li>
  );
}

/** Read-only view of a `describe` result: assembly/part tree, body ops, components, constraints. */
export function ComponentTree({ description, resolvedPorts }: Props) {
  const parts = new Map(description.parts.map((part) => [part.id, part]));
  const components = new Map(description.components.map((component) => [component.id, component]));
  const inTree = new Set<string>();
  const walk = (nodes: TreeNode[]) =>
    nodes.forEach((node) => {
      inTree.add(node.id);
      walk(node.children);
    });
  walk(description.tree);
  const loose = description.parts.filter((part) => !inTree.has(part.id));

  return (
    <section className="component-tree" aria-label="元件樹">
      <div className="validation-head">
        <strong>元件樹</strong>
        <span className="muted">
          {description.name} · 版本 {description.revision} · 根：
          <span className="mono">{description.computed_root ?? description.root ?? '—'}</span>
        </span>
      </div>

      {description.tree.length === 0 && loose.length === 0 ? (
        <p className="muted">文件中沒有零件或組件。</p>
      ) : (
        <ul className="ct-list ct-root" aria-label="組件與零件">
          {description.tree.map((node, index) => (
            <Node
              key={`${index}-${node.id}`}
              node={node}
              parts={parts}
              components={components}
              resolvedPorts={resolvedPorts}
            />
          ))}
          {loose.map((part, index) => (
            <Node
              key={`loose-${index}-${part.id}`}
              node={{ id: part.id, node: 'part', name: part.name, repeated: false, children: [] }}
              parts={parts}
              components={components}
              resolvedPorts={resolvedPorts}
            />
          ))}
        </ul>
      )}

      <h3 className="ct-heading">約束（{description.constraints.length}）</h3>
      {description.constraints.length === 0 ? (
        <p className="muted">沒有約束。</p>
      ) : (
        <ul className="ct-list ct-constraints" aria-label="約束">
          {description.constraints.map((constraint, index) => (
            <li key={`${index}-${constraint.id}`}>
              <span className="mono">{constraint.id}</span>{' '}
              <span className="ct-tag">{constraint.kind}</span>{' '}
              <span className="mono">
                {constraint.a}
                {constraint.b ? ` ↔ ${constraint.b}` : ''}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
