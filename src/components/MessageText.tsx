import React from "react";

/** 轻量内联渲染：**bold** 和 `code`，不引入完整 markdown 依赖 */
function renderInline(text: string): React.ReactNode {
  const parts = text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g);
  return parts.map((p, i) => {
    if (p.length > 4 && p.startsWith("**") && p.endsWith("**")) {
      return (
        <strong key={i} className="font-semibold text-zinc-100">
          {p.slice(2, -2)}
        </strong>
      );
    }
    if (p.length > 2 && p.startsWith("`") && p.endsWith("`")) {
      return (
        <code key={i} className="px-1 py-0.5 rounded bg-zinc-800/90 text-[12px] text-indigo-300">
          {p.slice(1, -1)}
        </code>
      );
    }
    return <React.Fragment key={i}>{p}</React.Fragment>;
  });
}

/** 按 ``` 切分代码块渲染消息文本，奇数段为代码块 */
export default function MessageText({ text }: { text: string }) {
  const blocks = text.split("```");
  return (
    <>
      {blocks.map((b, i) =>
        i % 2 === 1 ? (
          <pre
            key={i}
            className="my-1.5 overflow-x-auto rounded-lg border border-zinc-800/70 bg-zinc-950/80 px-2.5 py-2 text-[12px] leading-relaxed select-text"
          >
            {b.replace(/^[\w-]*\n/, "")}
          </pre>
        ) : (
          <span key={i}>{renderInline(b)}</span>
        )
      )}
    </>
  );
}
