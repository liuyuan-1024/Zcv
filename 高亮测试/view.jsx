import { useState } from "react";

function UserCard({ name, active = true }) {
  const [expanded, setExpanded] = useState(false);
  return (
    <article className="user-card" data-active={active}>
      <h2>{name}</h2>
      <Button disabled={!active} onClick={() => setExpanded(!expanded)}>
        {expanded ? "收起" : "展开"}
      </Button>
      {expanded && <p>欢迎使用 Zcv。</p>}
    </article>
  );
}

export default <UserCard name="Zcv" />;
