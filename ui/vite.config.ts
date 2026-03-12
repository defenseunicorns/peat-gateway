import { sveltekit } from '@sveltejs/kit/vite'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [sveltekit(), tailwindcss()],
  optimizeDeps: {
    include: [
      'carbon-icons-svelte/lib/Dashboard.svelte',
      'carbon-icons-svelte/lib/Building.svelte',
      'carbon-icons-svelte/lib/Network_3.svelte',
      'carbon-icons-svelte/lib/DataConnected.svelte',
      'carbon-icons-svelte/lib/Password.svelte',
      'carbon-icons-svelte/lib/Report.svelte',
      'carbon-icons-svelte/lib/ChevronDown.svelte',
      'carbon-icons-svelte/lib/ChevronRight.svelte',
      'carbon-icons-svelte/lib/Menu.svelte',
      'carbon-icons-svelte/lib/Close.svelte',
      'carbon-icons-svelte/lib/Locked.svelte',
      'carbon-icons-svelte/lib/CheckmarkFilled.svelte',
      'carbon-icons-svelte/lib/WarningAlt.svelte',
    ],
  },
  server: {
    proxy: {
      '/orgs': {
        target: 'http://localhost:8080',
        changeOrigin: true,
        secure: false,
      },
      '/health': {
        target: 'http://localhost:8080',
        changeOrigin: true,
        secure: false,
      },
      '/metrics': {
        target: 'http://localhost:8080',
        changeOrigin: true,
        secure: false,
      },
    },
  },
})
