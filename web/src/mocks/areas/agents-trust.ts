import type { TrustView } from "@/areas/agents/api";

// A small stand-in for the edge's trust-view renderer (design/AGENT-LAYER.md, Shape,
// `TrustView`, `Hidden`, `HIDDEN_CHARS`, `AuthorTrust`). The edge's renderer is the
// measured one; this one handles the constructs the fixtures use: HTML comments, hidden
// characters, link and image titles, title attributes, and closed <details>.

const HIDDEN_CHARS =
  /[\u00AD\u034F\u061C\u115F\u1160\u17B4\u17B5\u180E\u200B-\u200F\u202A-\u202E\u2060-\u2064\u2066-\u206F\u3164\uFEFF\uFFA0\uFFF9-\uFFFB]|\uDB40[\uDC00-\uDC7F]|\uDB40[\uDD00-\uDDEF]/g;

export type AuthorInput = Omit<TrustView["author"], "trusted">;

// `trusted`: a human who can push, never a bot or an agent App.
export function authorTrust(a: AuthorInput): TrustView["author"] {
  return { ...a, trusted: a.can_push && !a.is_bot && !a.is_agent };
}

export function trustView(text: string, author: AuthorInput): TrustView {
  const hidden = { html_comments: 0, invisible_chars: 0, link_title_text: 0, collapsed: 0 };
  let visible = text.replace(/<!--[\s\S]*?(-->|$)/g, () => {
    hidden.html_comments++;
    return "";
  });
  visible = visible.replace(
    /<details(?![^>]*\bopen\b)[^>]*>([\s\S]*?)(<\/details>|$)/gi,
    (_, inner) => {
      hidden.collapsed++;
      const summary = /<summary[^>]*>([\s\S]*?)<\/summary>/i.exec(inner)?.[1] ?? "";
      return summary.trim();
    },
  );
  visible = visible.replace(/(\]\([^)\s]+)\s+("[^"]*"|'[^']*')\)/g, (_, link) => {
    hidden.link_title_text++;
    return `${link})`;
  });
  visible = visible.replace(/\s(title|alt|aria-label)="[^"]*"/gi, () => {
    hidden.link_title_text++;
    return "";
  });
  visible = visible.replace(HIDDEN_CHARS, () => {
    hidden.invisible_chars++;
    return "";
  });
  return {
    visible: visible.replace(/\n{3,}/g, "\n\n").trim(),
    hidden,
    author: authorTrust(author),
  };
}
