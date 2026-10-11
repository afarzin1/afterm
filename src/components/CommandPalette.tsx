import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";

export interface Command {
  id: string;
  label: string;
  hint?: string;
  icon?: ReactNode;
  run(): void;
}

interface Props {
  commands: Command[];
  onClose(): void;
}

/** Subsequence match, so "nsh" finds "New SSH session". */
function matches(label: string, query: string): boolean {
  if (!query) return true;
  const haystack = label.toLowerCase();
  let index = 0;
  for (const character of query.toLowerCase()) {
    index = haystack.indexOf(character, index);
    if (index === -1) return false;
    index += 1;
  }
  return true;
}

export function CommandPalette({ commands, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLUListElement>(null);

  const results = useMemo(
    () => commands.filter(command => matches(command.label, query)),
    [commands, query]
  );

  useEffect(() => { setActive(0); }, [query]);

  useEffect(() => {
    listRef.current?.querySelector("li.active")?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const keydown = (event: React.KeyboardEvent) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive(index => Math.min(results.length - 1, index + 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive(index => Math.max(0, index - 1));
    } else if (event.key === "Enter") {
      event.preventDefault();
      const command = results[active];
      if (command) { onClose(); command.run(); }
    } else if (event.key === "Escape") {
      onClose();
    }
  };

  return <div className="palette-backdrop" onMouseDown={event => event.target === event.currentTarget && onClose()}>
    <div className="palette">
      <input
        autoFocus
        value={query}
        placeholder="Search or run a command"
        onChange={event => setQuery(event.target.value)}
        onKeyDown={keydown}
      />
      {results.length === 0
        ? <div className="empty">No matching command.</div>
        : <ul ref={listRef}>
            {results.map((command, index) => <li
              key={command.id}
              className={index === active ? "active" : ""}
              onMouseEnter={() => setActive(index)}
              onClick={() => { onClose(); command.run(); }}
            >
              {command.icon}
              {command.label}
              {command.hint && <small>{command.hint}</small>}
            </li>)}
          </ul>}
    </div>
  </div>;
}
