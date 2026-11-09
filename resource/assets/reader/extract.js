// Fleet Reader local extraction adapter. Publication HTML is parsed in a detached document.
// Readability is not a sanitizer: sanitize before parsing and again before rendering.
function extractArticle(input, customExtract) {
  const clean = DOMPurify.sanitize(input.html, {
    WHOLE_DOCUMENT: true,
    USE_PROFILES: { html: true },
    ADD_TAGS: ["title"],
    FORBID_TAGS: ["base", "style", "link", "iframe", "form", "input", "button"],
    FORBID_ATTR: ["style", "srcdoc"],
  });
  const doc = new DOMParser().parseFromString(clean, "text/html");
  const base = doc.createElement("base");
  base.href = input.url;
  doc.head.prepend(base);
  // A hook gets its own document: returning null can safely delegate to Mozilla even
  // after the hook has removed nodes. User-installed code is trusted, not sandboxed.
  const article = (customExtract && customExtract(doc.cloneNode(true), input.url, Readability))
    ?? new Readability(doc, { maxElemsToParse: 50000 }).parse();
  if (!article) return null;
  if (typeof article.content !== "string" || typeof article.then === "function") {
    throw new TypeError("extract must synchronously return an article with string content or null");
  }
  let content = DOMPurify.sanitize(article.content, {
    USE_PROFILES: { html: true },
    FORBID_TAGS: ["style", "link", "iframe", "form", "input", "button"],
    FORBID_ATTR: ["style", "srcdoc"],
  });
  const output = new DOMParser().parseFromString(content, "text/html");
  if (!output.body.textContent.trim()) return null;
  if (customExtract) {
    output.head.prepend(base.cloneNode(true));
    // Use the pinned Mozilla implementation's URL-only normalizer (including srcset).
    // Custom selectors often return innerHTML with relative links. These must resolve
    // against the final fetched page, even when inserted into an RSS preview after a redirect.
    new Readability(output)._fixRelativeUris(output.body);
    content = output.body.innerHTML;
  }
  return {
    content,
    title: typeof article.title === "string" ? article.title : null,
    byline: typeof article.byline === "string" ? article.byline : null,
    url: input.url,
  };
}
