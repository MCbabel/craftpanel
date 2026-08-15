<template>
	<RouterView v-if="ready" />
	<div v-else class="grid min-h-full place-items-center text-secondary">
		<SpinnerIcon class="size-8 animate-spin" />
	</div>
</template>

<script setup lang="ts">
import { SpinnerIcon } from '@modrinth/assets'
import { ref } from 'vue'
import { RouterView, useRouter } from 'vue-router'

const ready = ref(false)
const settle = () => {
	ready.value = true
}

useRouter().isReady().then(settle, settle)
</script>
