<script lang="ts">
  import Menu from 'carbon-icons-svelte/lib/Menu.svelte'
  import Close from 'carbon-icons-svelte/lib/Close.svelte'

  type Props = {
    sidebarOpen: boolean
    onToggle: () => void
    version?: string
  }

  let { sidebarOpen, onToggle, version = '0.1.0' }: Props = $props()
</script>

<nav
  class="fixed top-0 right-0 left-0 z-50 flex h-(--navbar-height) items-center justify-between border-b border-gray-800 bg-gray-900 px-4"
>
  <div class="flex items-center gap-3">
    <button
      class="rounded p-1 text-gray-400 hover:bg-gray-800 hover:text-white lg:hidden"
      onclick={onToggle}
      aria-label="Toggle sidebar"
    >
      {#if sidebarOpen}
        <Close size={24} />
      {:else}
        <Menu size={24} />
      {/if}
    </button>
    <a href="/_/orgs" class="flex items-center gap-2">
      <svg class="h-8 w-8 text-blue-500" viewBox="0 0 32 32" fill="currentColor">
        <circle cx="16" cy="16" r="6" />
        <circle cx="16" cy="4" r="3" />
        <circle cx="16" cy="28" r="3" />
        <circle cx="4" cy="16" r="3" />
        <circle cx="28" cy="16" r="3" />
        <line x1="16" y1="10" x2="16" y2="7" stroke="currentColor" stroke-width="1.5" />
        <line x1="16" y1="25" x2="16" y2="22" stroke="currentColor" stroke-width="1.5" />
        <line x1="10" y1="16" x2="7" y2="16" stroke="currentColor" stroke-width="1.5" />
        <line x1="25" y1="16" x2="22" y2="16" stroke="currentColor" stroke-width="1.5" />
      </svg>
      <span class="text-lg font-semibold text-white">Peat Gateway</span>
    </a>
  </div>
  <span class="text-xs text-gray-500">v{version}</span>
</nav>
