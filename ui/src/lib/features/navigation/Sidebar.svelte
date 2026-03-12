<script lang="ts">
  import { page } from '$app/state'
  import Building from 'carbon-icons-svelte/lib/Building.svelte'
  import ChevronDown from 'carbon-icons-svelte/lib/ChevronDown.svelte'
  import ChevronRight from 'carbon-icons-svelte/lib/ChevronRight.svelte'
  import DataConnected from 'carbon-icons-svelte/lib/DataConnected.svelte'
  import Network_3 from 'carbon-icons-svelte/lib/Network_3.svelte'
  import Password from 'carbon-icons-svelte/lib/Password.svelte'
  import Report from 'carbon-icons-svelte/lib/Report.svelte'
  import { fade, fly } from 'svelte/transition'

  import type { Organization } from '$lib/api/types'

  type Props = {
    orgs: Organization[]
    open: boolean
    onClose: () => void
  }

  let { orgs, open, onClose }: Props = $props()

  let expandedOrgs: Record<string, boolean> = $state({})

  let currentPath = $derived(page.url?.pathname ?? '')

  function toggleOrg(orgId: string) {
    expandedOrgs[orgId] = !expandedOrgs[orgId]
  }

  function isActive(path: string): boolean {
    return currentPath === path || currentPath.startsWith(path + '/')
  }

  interface NavItem {
    label: string
    href: string
    icon: typeof Network_3
  }

  function orgNav(orgId: string): NavItem[] {
    return [
      { label: 'Formations', href: `/_/orgs/${orgId}/formations`, icon: Network_3 },
      { label: 'CDC Sinks', href: `/_/orgs/${orgId}/sinks`, icon: DataConnected },
      { label: 'Tokens', href: `/_/orgs/${orgId}/tokens`, icon: Password },
      { label: 'Audit', href: `/_/orgs/${orgId}/audit`, icon: Report },
    ]
  }

  // Auto-expand org when navigating into it
  $effect(() => {
    for (const org of orgs) {
      if (isActive(`/_/orgs/${org.org_id}`)) {
        expandedOrgs[org.org_id] = true
      }
    }
  })
</script>

<!-- Mobile backdrop -->
{#if open}
  <button
    class="fixed inset-0 z-30 bg-black/60 lg:hidden"
    transition:fade={{ duration: 200 }}
    onclick={onClose}
    aria-label="Close sidebar"
  ></button>
{/if}

<aside
  class="fixed top--(--navbar-height) bottom-0 left-0 z-40 w-(--sidebar-width) overflow-y-auto border-r border-gray-800 bg-gray-950 transition-transform duration-200 lg:translate-x-0 {open
    ? 'translate-x-0'
    : '-translate-x-full'}"
>
  <nav class="p-3">
    <a
      href="/_/orgs"
      class="mb-3 flex items-center gap-2 rounded-lg px-3 py-2 text-sm font-medium transition-colors {currentPath ===
      '/_/orgs'
        ? 'bg-gray-800 text-white'
        : 'text-gray-400 hover:bg-gray-900 hover:text-white'}"
    >
      <Building size={16} />
      All Organizations
    </a>

    {#if orgs.length === 0}
      <p class="px-3 py-2 text-xs text-gray-600">No organizations</p>
    {/if}

    {#each orgs as org (org.org_id)}
      <div class="mb-1">
        <button
          class="flex w-full items-center justify-between rounded-lg px-3 py-2 text-sm transition-colors {isActive(
            `/_/orgs/${org.org_id}`,
          )
            ? 'bg-gray-800/50 text-white'
            : 'text-gray-400 hover:bg-gray-900 hover:text-white'}"
          onclick={() => toggleOrg(org.org_id)}
        >
          <span class="truncate font-medium">{org.display_name || org.org_id}</span>
          {#if expandedOrgs[org.org_id]}
            <ChevronDown size={16} />
          {:else}
            <ChevronRight size={16} />
          {/if}
        </button>

        {#if expandedOrgs[org.org_id]}
          <div class="ml-3 mt-1 space-y-0.5 border-l border-gray-800 pl-2">
            <a
              href="/_/orgs/{org.org_id}"
              class="flex items-center gap-2 rounded px-2 py-1.5 text-xs transition-colors {currentPath ===
              `/_/orgs/${org.org_id}`
                ? 'text-blue-400'
                : 'text-gray-500 hover:text-gray-300'}"
            >
              Overview
            </a>
            {#each orgNav(org.org_id) as item (item.href)}
              <a
                href={item.href}
                class="flex items-center gap-2 rounded px-2 py-1.5 text-xs transition-colors {isActive(
                  item.href,
                )
                  ? 'text-blue-400'
                  : 'text-gray-500 hover:text-gray-300'}"
              >
                <item.icon size={16} />
                {item.label}
              </a>
            {/each}
          </div>
        {/if}
      </div>
    {/each}
  </nav>
</aside>
