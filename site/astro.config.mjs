import { defineConfig } from 'astro/config';

// The teaser is one static page. The taste-skill kit under public/assets loads itself by
// import.meta.url, so Astro serves it untouched and inlines no scripts.
export default defineConfig({
  site: 'https://graftyard.com',
  build: { inlineStylesheets: 'never' },
});
