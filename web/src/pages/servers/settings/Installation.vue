<template>
	<InstallationSettingsLayout @reset-server="goToDangerZone" />
</template>

<script setup lang="ts">
import {
	injectModrinthClient,
	injectNotificationManager,
	InstallationSettingsLayout,
} from '@modrinth/ui'
import { useRouter } from 'vue-router'

import { useServerPage } from '@/composables/server-page'
import { useInstallationSettings } from '@/providers/installation-settings'

const router = useRouter()
const client = injectModrinthClient()
const { addNotification } = injectNotificationManager()
const { serverId, server, socket, context } = useServerPage()

function goToDangerZone(): void {
	void router.push({ name: 'server-settings', params: { id: serverId, section: 'general' } })
}

useInstallationSettings({
	serverId,
	server,
	socket,
	busyReasons: context.busyReasons,
	notify: addNotification,
	modrinth: client.labrinth.versions_v2,
})
</script>
