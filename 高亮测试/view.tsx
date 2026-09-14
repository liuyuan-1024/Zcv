import type { ReactNode } from "react";

type PanelProps = {
  title: string;
  children?: ReactNode;
  closable?: boolean;
};

export function Panel({ title, children, closable = true }: PanelProps) {
  return (
    <section className="panel" aria-label={title}>
      <header><strong>{title}</strong>{closable && <Button>关闭</Button>}</header>
      <div>{children ?? <em>暂无内容</em>}</div>
    </section>
  );
}

export const view = <Panel title="语法高亮">Hello, TSX!</Panel>;
