<script lang="ts">
  import Building from 'carbon-icons-svelte/lib/Building.svelte'
  import DataConnected from 'carbon-icons-svelte/lib/DataConnected.svelte'
  import Network_3 from 'carbon-icons-svelte/lib/Network_3.svelte'

  import type { CdcSinkConfig, FormationConfig, Organization } from '$lib/api/types'
  import { api } from '$lib/api/client'
  import EmptyState from '$components/EmptyState.svelte'

  let orgs: Organization[] = $state([])
  let orgFormations: Record<string, FormationConfig[]> = $state({})
  let orgSinks: Record<string, CdcSinkConfig[]> = $state({})
  let loading = $state(true)

  async function load() {
    try {
      orgs = await api.listOrgs()
      const results = await Promise.all(
        orgs.map(async (org) => {
          const [formations, sinks] = await Promise.all([
            api.listFormations(org.org_id).catch(() => []),
            api.listSinks(org.org_id).catch(() => []),
          ])
          return { orgId: org.org_id, formations, sinks }
        }),
      )
      for (const r of results) {
        orgFormations[r.orgId] = r.formations
        orgSinks[r.orgId] = r.sinks
      }
    } catch {
      orgs = []
    } finally {
      loading = false
    }
  }

  $effect(() => {
    load()
  })
</script>

<div>
  <div class="mb-6 flex items-center gap-3">
    <Building size={24} class="text-gray-400" />
    <h1 class="text-2xl font-semibold text-white">Organizations</h1>
  </div>

  {#if loading}
    <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      {#each Array(3) as _}
        <div class="card animate-pulse">
          <div class="mb-3 h-5 w-1/2 rounded bg-gray-800"></div>
          <div class="h-4 w-3/4 rounded bg-gray-800"></div>
        </div>
      {/each}
    </div>
  {:else if orgs.length === 0}
    <EmptyState message="No organizations found. Create one via the API to get started." />
  {:else}
    <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      {#each orgs as org (org.org_id)}
        <a
          href="/_/orgs/{org.org_id}"
          class="card transition-colors hover:border-gray-700"
        >
          <div class="mb-3 flex items-center justify-between">
            <h2 class="text-lg font-semibold text-white">
              {org.display_name || org.org_id}
            </h2>
            <span class="text-xs text-gray-500">{org.org_id}</span>
          </div>

          <div class="flex items-center gap-6 text-sm text-gray-400">
            <span class="flex items-center gap-1.5">
              <Network_3 size={16} />
              {orgFormations[org.org_id]?.length ?? 0} formations
            </span>
            <span class="flex items-center gap-1.5">
              <DataConnected size={16} />
              {orgSinks[org.org_id]?.length ?? 0} sinks
            </span>
          </div>

          <div class="mt-3 space-y-1.5">
            <div class="flex items-center justify-between text-xs">
              <span class="text-gray-500">Formations</span>
              <span class="text-gray-400"
                >{orgFormations[org.org_id]?.length ?? 0} / {org.quotas.max_formations}</span
              >
            </div>
            <div class="h-1.5 overflow-hidden rounded-full bg-gray-800">
              <div
                class="h-full rounded-full bg-blue-600"
                style="width: {Math.min(
                  100,
                  ((orgFormations[org.org_id]?.length ?? 0) / Math.max(1, org.quotas.max_formations)) * 100,
                )}%"
              ></div>
            </div>
          </div>
        </a>
      {/each}
    </div>
  {/if}
</div>
