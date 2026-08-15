<template>
	<div class="flex flex-col gap-6">
		<div class="isolate flex w-full select-none flex-col gap-4 overflow-auto">
			<NavTabs :links="tabs" />
		</div>

		<GeneralSection v-if="section === 'general'" />
		<InstallationSection v-else-if="section === 'installation'" />
		<NetworkSection v-else-if="section === 'network'" />
		<ServerSettingsPropertiesPage v-else-if="section === 'properties'" />
		<AdvancedSection v-else />
	</div>
</template>

<script setup lang="ts">
import {
	defineMessage,
	injectModrinthServerContext,
	NavTabs,
	useVIntl,
	provideServerSettings,
	ServerSettingsPropertiesPage,
	type ServerSettingsTabId,
	serverSettingsTabDefinitions,
} from '@modrinth/ui'
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import { useSession } from '@/composables/session'

import AdvancedSection from './settings/Advanced.vue'
import GeneralSection from './settings/General.vue'
import InstallationSection from './settings/Installation.vue'
import NetworkSection from './settings/Network.vue'

const { formatMessage } = useVIntl()
const route = useRoute()
const router = useRouter()
const { user, isAdmin } = useSession()
const { serverId, server } = injectModrinthServerContext()

const SECTIONS: ServerSettingsTabId[] = [
	'general',
	'installation',
	'network',
	'properties',
	'advanced',
]

function isSection(value: unknown): value is ServerSettingsTabId {
	return typeof value === 'string' && SECTIONS.includes(value as ServerSettingsTabId)
}

const section = computed<ServerSettingsTabId>(() =>
	isSection(route.params.section) ? route.params.section : 'general',
)

watch(
	() => route.params.section,
	(value) => {
		if (route.name !== 'server-settings' || isSection(value)) return
		void router.replace({ name: 'server-settings', params: { section: 'general' } })
	},
	{ immediate: true },
)

const isProxy = computed(() => server.value?.loader?.toLowerCase() === 'velocity')

const tabs = computed(() =>
	serverSettingsTabDefinitions.map((tab) => ({
		label: formatMessage(
			defineMessage({ id: `server.settings.tabs.${tab.id}`, defaultMessage: tab.label }),
		),
		icon: tab.icon,
		href: `/servers/${serverId}/settings/${tab.id}`,
		shown:
			(tab.shown?.({
				serverId,
				ownerId: server.value?.owner_id ?? '',
				serverStatus: server.value?.status,
				isOwner: server.value?.owner_id === user.value?.id,
				isAdmin: isAdmin.value,
			}) ?? true) &&
			(tab.id !== 'properties' || !isProxy.value),
	})),
)

provideServerSettings({
	isApp: ref(false),
	currentUserId: computed(() => user.value?.id ?? null),
	currentUserRole: computed(() => user.value?.panel_role ?? null),
	browseModpacks: () => undefined,
})
</script>
