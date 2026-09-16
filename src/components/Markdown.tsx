import { memo } from "react";
import ReactMarkdown from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import remarkGfm from "remark-gfm";

import { invoke } from "@tauri-apps/api/core";

/**
 * GFM + syntax highlighting. No rehype-raw — model HTML is escaped (XSS-safe).
 * External links route through the opener plugin so they open in the system
 * browser instead of navigating the webview away.
 */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      rehypePlugins={[rehypeHighlight]}
      components={{
        a: ({ href, children }) => (
          <a
            href={href}
            onClick={(e) => {
              if (!href) return;
              e.preventDefault();
              if (/^https?:\/\//i.test(href)) {
                invoke("open_external", { url: href }).catch(() => {
                  /* chip-less failure is fine in M0 */
                });
              }
            }}
          >
            {children}
          </a>
        ),
      }}
    >
      {text}
    </ReactMarkdown>
  );
});