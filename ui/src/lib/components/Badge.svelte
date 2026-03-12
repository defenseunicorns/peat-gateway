<script lang="ts">
  import type { EnrollmentPolicy } from '$lib/api/types'

  import type { Snippet } from 'svelte'

  type Props = {
    variant?: 'green' | 'yellow' | 'red' | 'blue' | 'gray'
    policy?: EnrollmentPolicy
    class?: string
    children?: Snippet
  }

  const policyVariant: Record<EnrollmentPolicy, 'green' | 'yellow' | 'red'> = {
    Open: 'green',
    Controlled: 'yellow',
    Strict: 'red',
  }

  let { variant, policy, class: className, children }: Props = $props()

  let resolved = $derived(policy ? policyVariant[policy] : (variant ?? 'gray'))

  const colors: Record<string, string> = {
    green: 'bg-green-900/50 text-green-400 border-green-800',
    yellow: 'bg-yellow-900/50 text-yellow-400 border-yellow-800',
    red: 'bg-red-900/50 text-red-400 border-red-800',
    blue: 'bg-blue-900/50 text-blue-400 border-blue-800',
    gray: 'bg-gray-800 text-gray-400 border-gray-700',
  }
</script>

<span
  class="inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-medium {colors[resolved]} {className ?? ''}"
>
  {#if children}{@render children()}{/if}
</span>
