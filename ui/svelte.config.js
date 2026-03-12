import adapter from '@sveltejs/adapter-static'
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte'
import { optimizeImports } from 'carbon-preprocess-svelte'

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: [vitePreprocess(), optimizeImports()],

  kit: {
    adapter: adapter({
      fallback: 'index.html',
      strict: true,
    }),

    paths: {
      base: '/_',
    },

    alias: {
      $components: './src/lib/components',
      $features: './src/lib/features',
      $styles: './src/styles',
    },
  },
}

export default config
