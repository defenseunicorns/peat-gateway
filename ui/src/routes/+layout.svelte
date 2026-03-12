<script lang="ts">
  import '$styles/app.css'

  import Root from '$features/layout/Root.svelte'

  import type { Organization } from '$lib/api/types'
  import { api } from '$lib/api/client'

  let { children } = $props()

  let orgs: Organization[] = $state([])

  async function loadOrgs() {
    try {
      orgs = await api.listOrgs()
    } catch {
      orgs = []
    }
  }

  $effect(() => {
    loadOrgs()
  })
</script>

<Root {orgs}>
  {@render children?.()}
</Root>
