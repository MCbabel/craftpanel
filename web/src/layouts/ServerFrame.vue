<template>
	<div class="flex flex-col gap-4">
		<PageHeader :title="server.name" :divider="false" :bottom-padding="false">
			<template #leading>
				<ServerIcon :image="undefined" />
			</template>

			<template #badges>
				<Badge :type="powerState" :color="STATE_COLORS[powerState]" />
				<CopyCode :text="address" />
			</template>

			<template #metadata>
				<ServerInfoLabels
					:server-data="vendorServer"
					:show-game-label="server.game_version !== null"
					:show-loader-label="server.loader !== null"
					:uptime-seconds="uptimeSeconds"
					class="flex w-full flex-row flex-wrap items-center gap-2 text-secondary"
				/>
			</template>

			<template #actions>
				<PageHeaderActions>
					<PanelServerActionButton :disabled="installError !== null" />
				</PageHeaderActions>
			</template>
		</PageHeader>

		<div class="isolate flex w-full select-none flex-col gap-4 overflow-auto">
			<NavTabs :links="tabs" replace />
		</div>

		<ServerNotice
			v-if="notice"
			:level="notice.level"
			:message="notice.message"
			:dismissable="false"
		/>

		<Admonition
			v-if="connection === 'reconnecting'"
			type="warning"
			:header="formatMessage(messages.reconnectingTitle)"
			:body="formatMessage(messages.reconnectingBody)"
		/>
		<Admonition
			v-else-if="connection === 'lost'"
			type="critical"
			:header="formatMessage(messages.disconnectedTitle)"
			:body="formatMessage(messages.disconnectedBody)"
		>
			<template #actions>
				<Button type="colored" color="red" @click="socket.connect()">
					<UpdatedIcon />
					{{ formatMessage(messages.reconnect) }}
				</Button>
			</template>
		</Admonition>

		<ServerPanelAdmonitions
			:sync-progress="installProgress"
			:content-error="installError"
			@content-retry="retryInstall"
		/>

		<RouterView />
	</div>
</template>

<script setup lang="ts">
import {
	BoxesIcon,
	DatabaseBackupIcon,
	FolderOpenIcon,
	LayoutTemplateIcon,
	SettingsIcon,
	UpdatedIcon,
	UsersIcon,
} from '@modrinth/assets'
import {
	Admonition,
	Badge,
	Button,
	commonMessages,
	CopyCode,
	defineMessages,
	injectNotificationManager,
	NavTabs,
	PageHeader,
	PageHeaderActions,
	PanelServerActionButton,
	ServerIcon,
	ServerInfoLabels,
	ServerNotice,
	ServerPanelAdmonitions,
	useVIntl,
} from '@modrinth/ui'
import { computed, onScopeDispose, watch } from 'vue'
import { RouterView, useRoute, useRouter } from 'vue-router'

import { api, isApiRequestError, type PowerState, type Server, ServerSocket } from '@/api'
import { publicAddress } from '@/api/playit'
import { provideArchonAdapters } from '@/composables/archon-adapters'
import { useServerTunnel } from '@/composables/playit'
import { provideServerPage } from '@/composables/server-page'
import { useSession } from '@/composables/session'
import { provideServerContext } from '@/providers'
import { useConsoleManager } from '@/providers/console-manager'

import { serverNotice } from './server-notice'

const props = defineProps<{ server: Server }>()

const { formatMessage } = useVIntl()
const { addNotification } = injectNotificationManager()
const route = useRoute()
const router = useRouter()
const session = useSession()

const socket = new ServerSocket(props.server.id)
const page = provideServerContext({ server: props.server, socket })
provideServerPage({ ...page, serverId: props.server.id, socket })

provideArchonAdapters(props.server.id)
socket.connect()
onScopeDispose(() => socket.close())

const { context, installError, installOperation, installProgress, server, socketStatus } = page
const { operations, refreshOperations } = page
const { powerState, uptimeSeconds } = context
const vendorServer = context.server

useConsoleManager({
	serverId: props.server.id,
	socket,
	powerState: () => powerState.value,
	permissions: () => server.value.current_user_permissions,
})

const STATE_COLORS: Record<PowerState, string> = {
	stopped: 'gray',
	starting: 'orange',
	running: 'green',
	stopping: 'orange',
	crashed: 'red',
}

const messages = defineMessages({
	overview: {
		id: 'panel.server.tabs.overview',
		defaultMessage: 'Overview',
	},
	files: {
		id: 'panel.server.tabs.files',
		defaultMessage: 'Files',
	},
	backups: {
		id: 'panel.server.tabs.backups',
		defaultMessage: 'Backups',
	},
	access: {
		id: 'panel.server.tabs.access',
		defaultMessage: 'Access',
	},
	setupPending: {
		id: 'panel.server.notice.setup-pending',
		defaultMessage:
			'This server has no installation yet. Pick a loader and a version under Settings.',
	},
	broken: {
		id: 'panel.server.notice.broken',
		defaultMessage:
			'The last installation did not finish, so this server cannot start. Install it again ' +
			'under Settings.',
	},
	deleting: {
		id: 'panel.server.notice.deleting',
		defaultMessage: 'This server is being deleted.',
	},
	deleteFailed: {
		id: 'panel.server.notice.delete-failed',
		defaultMessage:
			'This server could not be deleted: {reason} Its files are still there and still count ' +
			'against your disk. Delete it again under Settings.',
	},
	reconnectingTitle: {
		id: 'panel.server.connection.reconnecting.title',
		defaultMessage: 'Reconnecting to the server',
	},
	reconnectingBody: {
		id: 'panel.server.connection.reconnecting.body',
		defaultMessage: 'The live connection dropped. Console and metrics stand still until it is back.',
	},
	disconnectedTitle: {
		id: 'panel.server.connection.lost.title',
		defaultMessage: 'No live connection',
	},
	disconnectedBody: {
		id: 'panel.server.connection.lost.body',
		defaultMessage: 'Console, metrics and progress are no longer updating.',
	},
	reconnect: {
		id: 'panel.server.connection.reconnect',
		defaultMessage: 'Reconnect',
	},
	accessRemoved: {
		id: 'panel.server.gone.access-removed',
		defaultMessage: 'Your access to this server was removed.',
	},
	deleted: {
		id: 'panel.server.gone.deleted',
		defaultMessage: 'This server has been deleted.',
	},
	retryFailed: {
		id: 'panel.server.install.retry-failed',
		defaultMessage: 'The installation could not be started again.',
	},
})

const base = computed(() => `/servers/${encodeURIComponent(server.value.id)}`)
const setupPending = computed(() => server.value.flows.intro)

const tabs = computed(() => [
	{
		label: formatMessage(messages.overview),
		href: base.value,
		icon: LayoutTemplateIcon,
	},
	{
		label: formatMessage(commonMessages.contentLabel),
		href: `${base.value}/content`,
		icon: BoxesIcon,
	},
	{
		label: formatMessage(messages.files),
		href: `${base.value}/files`,
		icon: FolderOpenIcon,
	},
	{
		label: formatMessage(messages.backups),
		href: `${base.value}/backups`,
		icon: DatabaseBackupIcon,
	},
	{
		label: formatMessage(messages.access),
		href: `${base.value}/access`,
		icon: UsersIcon,
	},
	{
		label: formatMessage(commonMessages.settingsLabel),
		href: `${base.value}/settings`,
		icon: SettingsIcon,
	},
])

const notice = computed(() => {
	const seen = serverNotice(server.value.status, setupPending.value, operations.value)
	switch (seen?.kind) {
		case 'delete-failed':
			return {
				level: 'critical',
				message: formatMessage(messages.deleteFailed, { reason: seen.reason }),
			}
		case 'deleting':
			return { level: 'critical', message: formatMessage(messages.deleting) }
		case 'broken':
			return { level: 'warn', message: formatMessage(messages.broken) }
		case 'setup-pending':
			return { level: 'info', message: formatMessage(messages.setupPending) }
		default:
			return null
	}
})

const { tunnel } = useServerTunnel(props.server.id)
const address = computed(() => {
	const { ip, port } = server.value.net
	return publicAddress(tunnel.value) ?? `${ip ?? location.hostname}:${port}`
})

const GONE_CODES = new Set([4401, 4403, 4404])

const connection = computed(() => {
	const status = socketStatus.value
	if (status.phase === 'open') return 'connected'
	if (status.closeCode !== null && GONE_CODES.has(status.closeCode)) return 'gone'
	if (status.givenUp || (status.phase === 'closed' && status.closeCode !== null)) return 'lost'
	return status.attempts > 0 ? 'reconnecting' : 'connecting'
})

watch(socketStatus, (status) => {
	if (status.closeCode === null || !GONE_CODES.has(status.closeCode)) return

	if (status.closeCode === 4401) {
		session.forget()
		void router.replace({ name: 'login', query: { redirect: route.fullPath } })
		return
	}

	addNotification({
		title: formatMessage(status.closeCode === 4404 ? messages.deleted : messages.accessRemoved),
		text: server.value.name,
		type: 'warning',
	})
	void router.replace({ name: 'servers' })
})

async function retryInstall(): Promise<void> {
	const operation = installOperation.value
	if (operation === null) return

	try {
		await api.operations.retry(server.value.id, operation.id)
		await refreshOperations()
	} catch (error) {
		addNotification({
			title: formatMessage(messages.retryFailed),
			text: isApiRequestError(error) ? error.message : String(error),
			type: 'error',
		})
	}
}
</script>
