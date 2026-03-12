<script lang="ts">
  import type { Snippet } from 'svelte'

  import type { Organization } from '$lib/api/types'
  import Navbar from '$features/navigation/Navbar.svelte'
  import Sidebar from '$features/navigation/Sidebar.svelte'

  type Props = {
    orgs: Organization[]
    children: Snippet
  }

  let { orgs, children }: Props = $props()

  let sidebarOpen = $state(false)

  function toggleSidebar() {
    sidebarOpen = !sidebarOpen
  }

  function closeSidebar() {
    sidebarOpen = false
  }
</script>

<Navbar {sidebarOpen} onToggle={toggleSidebar} />
<Sidebar {orgs} open={sidebarOpen} onClose={closeSidebar} />

<main
  class="mt-(--navbar-height) ml-0 min-h-[calc(100vh-var(--navbar-height))] p-6 lg:ml-(--sidebar-width)"
>
  {@render children()}
</main>
