import React from "react";

/** 轻量内联渲染：**bold** 和 `code`，不引入完整 markdown 依赖 */
function renderInline(text: string): React.ReactNode {
  const parts = text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g);
  return parts.map((p, i) => {
    if (p.length > 4 && p.startsWith("**") && p.endsWith("**")) {
      return (
        <strong key={i} className="font-semibold text-ink">
          {p.slice(2, -2)}
        </strong>
      );
    }
    if (p.length > 2 && p.startsWith("`") && p.endsWith("`")) {
      return (
        <code key={i} className="px-1 py-0.5 rounded bg-black/50 text-[12px] text-accent">
          {p.slice(1, -1)}
        </code>
      );
    }
    return <React.Fragment key={i}>{p}</React.Fragment>;
  });
}

/** 常见 info string 语言名：只有命中才删除首行，无语言代码块的首行必须保留 */
const LANGS = new Set([
  "rust", "rs", "js", "javascript", "ts", "typescript", "tsx", "jsx",
  "python", "py", "bash", "sh", "zsh", "shell", "json", "jsonl", "yaml", "yml",
  "toml", "xml", "html", "css", "scss", "sql", "go", "java", "c", "h",
  "cpp", "cc", "cs", "csharp", "swift", "kotlin", "kt", "rb", "ruby",
  "md", "markdown", "text", "plain", "plaintext", "dockerfile", "diff",
]);

/** 按 ``` 切分代码块渲染消息文本，奇数段为代码块 */
export default function MessageText({ text }: { text: string }) {
  const blocks = text.split("```");
  return (
    <>
      {blocks.map((b, i) => {
        if (i % 2 === 1) {
          let code = b;
          const nl = code.indexOf("\n");
          if (nl > 0) {
            const first = code.slice(0, nl).trim().toLowerCase();
            if (LANGS.has(first)) {
              code = code.slice(nl + 1);
            }
          }
          return (
            <pre
              key={i}
              className="my-1.5 overflow-x-auto rounded-lg border border-line bg-page px-2.5 py-2 text-[12px] leading-relaxed select-text"
            >
              {code}
            </pre>
          );
        }
        return <span key={i}>{renderInline(b)}</span>;
      })}
    </>
  );
}
