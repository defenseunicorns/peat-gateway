<script lang="ts">
  import { page } from '$app/state'

  import type { FormationConfig } from '$lib/api/types'
  import { api, ApiError } from '$lib/api/client'
  import Badge from '$components/Badge.svelte'
  import DataTable from '$components/DataTable.svelte'
  import EmptyState from '$components/EmptyState.svelte'

  let orgId = $derived(page.params.org_id)
  let appId = $derived(page.params.app_id)

  let formation: FormationConfig | null = $state(null)
  let peers: unknown[] = $state([])
  let documents: unknown[] = $state([])
  let certificates: unknown[] = $state([])
  let loading = $state(true)
  let error: string | null = $state(null)

  async function load(oid: string, aid: string) {
    loading = true
    error = null
    try {
      const [f, p, d, c] = await Promise.all([
        api.getFormation(oid, aid),
        api.listPeers(oid, aid).catch(() => []),
        api.listDocuments(oid, aid).catch(() => []),
        api.listCertificates(oid, aid).catch(() => []),
      ])
      formation = f
      peers = p
      documents = d
      certificates = c
    } catch (e) {
      error = e instanceof ApiError ? e.message : 'Failed to load formation'
    } finally {
      loading = false
    }
  }

  $effect(() => {
    if (orgId && appId) load(orgId, appId)
  })
</script>

{#if loading}
  <div class="animate-pulse space-y-4">
    <div class="h-8 w-48 rounded bg-gray-800"></div>
    <div class="card h-32"></div>
  </div>
{:else if error}
  <div class="rounded-lg bg-red-900/30 p-4 text-red-200">{error}</div>
{:else if formation}
  <div class="mb-6">
    <div class="flex items-center gap-3">
      <h1 class="text-2xl font-semibold text-white">{formation.app_id}</h1>
      <Badge policy={formation.enrollment_policy}>{formation.enrollment_policy}</Badge>
    </div>
    <p class="mt-1 text-sm text-gray-500">
      <a href="/_/orgs/{orgId}" class="hover:text-gray-300">{orgId}</a>
    </p>
  </div>

  <!-- Mesh info -->
  <div class="card mb-6">
    <h2 class="mb-3 text-sm font-medium text-gray-400 uppercase">Mesh Info</h2>
    <div class="grid gap-4 sm:grid-cols-2">
      <div>
        <p class="text-xs text-gray-500">Mesh ID</p>
        <p class="font-mono text-sm text-white">{formation.mesh_id}</p>
      </div>
      <div>
        <p class="text-xs text-gray-500">Enrollment Policy</p>
        <p class="text-sm text-white">{formation.enrollment_policy}</p>
      </div>
    </div>
  </div>

  <!-- Peers -->
  <div class="mb-6">
    <h2 class="mb-3 text-lg font-medium text-white">Peers</h2>
    {#if peers.length === 0}
      <EmptyState message="No peers connected" />
    {:else}
      <DataTable columns={['Peer ID', 'Tier', 'Status']}>
        {#each peers as peer, i}
          <tr class="table-row">
            <td class="data-cell font-mono text-xs text-white">{JSON.stringify(peer)}</td>
            <td class="data-cell">-</td>
            <td class="data-cell">-</td>
          </tr>
        {/each}
      </DataTable>
    {/if}
  </div>

  <!-- Documents -->
  <div class="mb-6">
    <h2 class="mb-3 text-lg font-medium text-white">Documents</h2>
    {#if documents.length === 0}
      <EmptyState message="No documents" />
    {:else}
      <DataTable columns={['Document ID', 'Fields']}>
        {#each documents as doc, i}
          <tr class="table-row">
            <td class="data-cell font-mono text-xs text-white">{JSON.stringify(doc)}</td>
            <td class="data-cell">-</td>
          </tr>
        {/each}
      </DataTable>
    {/if}
  </div>

  <!-- Certificates -->
  <div>
    <h2 class="mb-3 text-lg font-medium text-white">Certificates</h2>
    {#if certificates.length === 0}
      <EmptyState message="No certificates issued" />
    {:else}
      <DataTable columns={['Certificate', 'Issued', 'Expires']}>
        {#each certificates as cert, i}
          <tr class="table-row">
            <td class="data-cell font-mono text-xs text-white">{JSON.stringify(cert)}</td>
            <td class="data-cell">-</td>
            <td class="data-cell">-</td>
          </tr>
        {/each}
      </DataTable>
    {/if}
  </div>
{/if}
