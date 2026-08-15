<template>
	<div class="flex flex-col gap-6">
		<ServerManageStats :data="isConnected ? stats : undefined" :loading="!isConnected" />

		<PlayitAddress />

		<div class="flex min-h-[700px] flex-col gap-2">
			<h2 class="m-0 shrink-0 text-2xl font-semibold text-contrast">
				{{ formatMessage(messages.console) }}
			</h2>
			<ConsolePageLayout />
		</div>
	</div>
</template>

<script setup lang="ts">
import { ConsolePageLayout, defineMessages, useVIntl } from '@modrinth/ui'
import ServerManageStats from '@modrinth/ui/src/components/servers/ServerManageStats.vue'

import PlayitAddress from '@/components/PlayitAddress.vue'
import { useServerPage } from '@/composables/server-page'

const { formatMessage } = useVIntl()
const { context } = useServerPage()
const { isConnected, stats } = context

const messages = defineMessages({
	console: {
		id: 'panel.server.console',
		defaultMessage: 'Console',
	},
})
</script>
