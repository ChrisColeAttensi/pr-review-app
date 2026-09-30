import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import { openUrl } from "@tauri-apps/plugin-opener";

// GitHub bodies mix markdown with HTML (<details>, <img>, bot linkbacks). Render it,
// but through GitHub's own allow-list so nothing executable gets in.
const schema = {
  ...defaultSchema,
  attributes: { ...defaultSchema.attributes, img: [...(defaultSchema.attributes?.img ?? []), "width", "height"] },
};

export function Markdown({ children, className }: { children: string; className?: string }) {
  return (
    <div className={`md ${className ?? ""}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeRaw, [rehypeSanitize, schema]]}
        components={{
          a: ({ href, children }) => (
            <a
              href={href}
              onClick={(e) => {
                e.preventDefault();
                if (href) openUrl(href);
              }}
            >
              {children}
            </a>
          ),
        }}
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}
