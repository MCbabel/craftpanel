<template>
	<div class="flex flex-col gap-6">
		<div class="flex flex-wrap items-center gap-3">
			<h1 class="m-0 mr-auto text-2xl font-extrabold text-contrast">
				{{ formatMessage(commonMessages.serversLabel) }}
			</h1>

			<StyledInput
				v-if="servers.length > 0"
				v-model="query"
				type="search"
				:icon="SearchIcon"
				:placeholder="formatMessage(messages.searchPlaceholder)"
				clearable
				wrapper-class="w-full sm:w-72"
			/>

			<ButtonLink
				v-if="capabilities?.can_create_servers"
				:to="{ name: 'server-new' }"
				type="colored"
				color="brand"
			>
				<PlusIcon aria-hidden="true" />
				{{ formatMessage(messages.newServer) }}
			</ButtonLink>
			<Button v-else-if="capabilities" type="colored" color="brand" disabled>
				<PlusIcon aria-hidden="true" />
				{{ formatMessage(messages.newServer) }}
			</Button>
		</div>

		<Admonition v-if="blocked" type="warning" :body="blocked" />

		<Admonition v-if="failure" type="critical" :header="formatMessage(messages.loadFailed)">
			{{ failure }}
			<template #actions>
				<Button @click="() => load(true)">
					<RotateCounterClockwiseIcon aria-hidden="true" />
					{{ formatMessage(messages.retry) }}
				</Button>
			</template>
		</Admonition>

		<LoadingIndicator v-else-if="loading" />

		<EmptyState
			v-else-if="servers.length === 0"
			type="no-tasks"
			:heading="formatMessage(messages.emptyHeading)"
			:description="formatMessage(messages.emptyDescription)"
		>
			<template v-if="capabilities?.can_create_servers" #actions>
				<ButtonLink :to="{ name: 'server-new' }" type="colored" color="brand">
					<ServerPlusIcon aria-hidden="true" />
					{{ formatMessage(messages.newServer) }}
				</ButtonLink>
			</template>
		</EmptyState>

		<EmptyState
			v-else-if="groups.length === 0"
			type="no-search-result"
			:heading="formatMessage(messages.noMatchHeading)"
			:description="formatMessage(messages.noMatchDescription, { query })"
		>
			<template #actions>
				<Button @click="query = ''">
					<XIcon aria-hidden="true" />
					{{ formatMessage(commonMessages.clearButton) }}
				</Button>
			</template>
		</EmptyState>

		<section v-for="group in groups" :key="group.key" class="flex flex-col gap-3">
			<h2 class="m-0 text-lg font-semibold text-secondary">{{ group.title }}</h2>

			<SmartClickable v-for="row in group.rows" :key="row.server.id">
				<template #clickable>
					<RouterLink
						:to="{ name: 'server-overview', params: { id: row.server.id } }"
						:aria-label="row.server.name"
						class="rounded-2xl"
					/>
				</template>

				<div
					class="smart-clickable:highlight-on-hover smart-clickable:outline-on-focus flex flex-row items-center gap-4 overflow-x-hidden rounded-2xl border-[1px] border-solid border-surface-4 bg-bg-raised p-4"
				>
					<ServerIcon :image="undefined" :disabled="row.server.status !== 'available'" />

					<div class="flex min-w-0 flex-col gap-1.5">
						<div class="flex flex-row flex-wrap items-center gap-2.5">
							<h3 class="m-0 truncate text-xl font-bold text-contrast">{{ row.server.name }}</h3>

							<div
								v-if="group.key === 'shared'"
								class="flex min-w-0 items-center gap-1 rounded-full border border-solid border-surface-5 bg-surface-4 px-2 py-1 pr-2.5 text-sm font-medium text-primary"
							>
								<Avatar
									:src="row.owner?.avatar_url"
									:tint-by="row.owner?.username"
									size="1.25rem"
									circle
									no-shadow
								/>
								<span class="max-w-32 truncate">
									{{ row.owner?.username ?? formatMessage(commonMessages.unknownLabel) }}
								</span>
							</div>

							<Badge :type="row.state.label" :color="row.state.color" />
						</div>

						<ServerInfoLabels
							:server-data="{
								game: row.server.game,
								mc_version: row.server.game_version,
								loader: row.loaderName,
								loader_version: row.server.loader_version,
								net: row.server.net,
							}"
							:server-id="row.server.id"
							show-game-label
							:show-loader-label="!!row.loaderName"
							:linked="false"
							class="flex w-full flex-row flex-wrap items-center gap-2 text-primary"
						/>

						<div class="flex flex-row flex-wrap items-center gap-2 text-sm text-secondary">
							<CopyCode
								v-if="row.address"
								class="smart-clickable:allow-pointer-events"
								:text="row.address"
							/>
							<span v-else class="font-medium">
								{{ formatMessage(messages.portOnly, { port: row.server.net.port }) }}
							</span>
							<StatItem>
								<MemoryStickIcon aria-hidden="true" class="size-4" />
								{{ formatBytes(row.server.memory_mib * 1024 * 1024, 0) }}
							</StatItem>
							<StatItem>
								<DatabaseBackupIcon aria-hidden="true" class="size-4" />
								{{ row.server.used_backup_quota }} / {{ row.server.backup_quota }}
							</StatItem>
						</div>
					</div>

					<div v-if="row.operation" class="ml-auto hidden w-52 shrink-0 md:block">
						<ProgressBar
							full-width
							show-progress
							:progress="row.operation.progress"
							:waiting="row.operation.state === 'queued'"
							:label="formatMessage(operationLabels[row.operation.kind])"
							label-class="text-sm font-medium text-secondary"
						/>
					</div>
				</div>
			</SmartClickable>
		</section>
	</div>
</template>

<script setup lang="ts">
import {
	DatabaseBackupIcon,
	MemoryStickIcon,
	PlusIcon,
	RotateCounterClockwiseIcon,
	SearchIcon,
	ServerPlusIcon,
	XIcon,
} from '@modrinth/assets'
import {
	Admonition,
	Avatar,
	Badge,
	Button,
	ButtonLink,
	commonMessages,
	CopyCode,
	defineMessages,
	EmptyState,
	LoadingIndicator,
	type MessageDescriptor,
	ProgressBar,
	ServerIcon,
	ServerInfoLabels,
	SmartClickable,
	StatItem,
	StyledInput,
	useFormatBytes,
	useVIntl,
} from '@modrinth/ui'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'

import {
	api,
	type BusyReasonCode,
	isApiRequestError,
	type Operation,
	type OperationKind,
	type Server,
	type Ulid,
	type UserRef,
} from '@/api'
import { useSession } from '@/composables/session'

const POLL_INTERVAL_MS = 5_000

const { formatMessage } = useVIntl()
const formatBytes = useFormatBytes()
const { user } = useSession()

const messages = defineMessages({
	newServer: {
		id: 'panel.servers.new-server',
		defaultMessage: 'New server',
	},
	searchPlaceholder: {
		id: 'panel.servers.search-placeholder',
		defaultMessage: 'Search servers...',
	},
	yourServers: {
		id: 'panel.servers.group.own',
		defaultMessage: 'Your servers',
	},
	sharedWithYou: {
		id: 'panel.servers.group.shared',
		defaultMessage: 'Shared with you',
	},
	emptyHeading: {
		id: 'panel.servers.empty.heading',
		defaultMessage: 'No servers yet',
	},
	emptyDescription: {
		id: 'panel.servers.empty.description',
		defaultMessage: 'Pick a loader and a version, and this machine will run it in a minute.',
	},
	noMatchHeading: {
		id: 'panel.servers.no-match.heading',
		defaultMessage: 'Nothing matches',
	},
	noMatchDescription: {
		id: 'panel.servers.no-match.description',
		defaultMessage: 'No server matches “{query}”.',
	},
	loadFailed: {
		id: 'panel.servers.load-failed',
		defaultMessage: 'The server list could not be loaded',
	},
	retry: {
		id: 'panel.servers.retry',
		defaultMessage: 'Try again',
	},
	portOnly: {
		id: 'panel.servers.port-only',
		defaultMessage: 'Port {port}',
	},
	unreachable: {
		id: 'panel.servers.error.unreachable',
		defaultMessage: 'The panel could not be reached.',
	},
	overLimit: {
		id: 'panel.servers.blocked.over-limit',
		defaultMessage:
			'Your servers already claim your whole memory budget. Free some up before making another one.',
	},
	systemUserNotReady: {
		id: 'panel.servers.blocked.system-user',
		defaultMessage:
			'Your system account is not ready yet. Nothing can run until an administrator has fixed it.',
	},
	installing: {
		id: 'panel.servers.state.installing',
		defaultMessage: 'Installing',
	},
	deleting: {
		id: 'panel.servers.state.deleting',
		defaultMessage: 'Deleting',
	},
	broken: {
		id: 'panel.servers.state.broken',
		defaultMessage: 'Broken',
	},
	setupPending: {
		id: 'panel.servers.state.setup-pending',
		defaultMessage: 'Needs setup',
	},
	ready: {
		id: 'panel.servers.state.ready',
		defaultMessage: 'Ready',
	},
	syncingContent: {
		id: 'panel.servers.state.syncing-content',
		defaultMessage: 'Syncing content',
	},
	backingUp: {
		id: 'panel.servers.state.backup-creating',
		defaultMessage: 'Backing up',
	},
	restoring: {
		id: 'panel.servers.state.backup-restoring',
		defaultMessage: 'Restoring',
	},
})

const operationLabels: Record<OperationKind, MessageDescriptor> = defineMessages({
	server_create: { id: 'panel.operation.server-create', defaultMessage: 'Setting up' },
	server_delete: { id: 'panel.operation.server-delete', defaultMessage: 'Deleting' },
	install_loader: { id: 'panel.operation.install-loader', defaultMessage: 'Installing loader' },
	repair_content: { id: 'panel.operation.repair-content', defaultMessage: 'Repairing' },
	reset_server: { id: 'panel.operation.reset-server', defaultMessage: 'Resetting' },
	install_modpack: { id: 'panel.operation.install-modpack', defaultMessage: 'Installing modpack' },
	install_content: { id: 'panel.operation.install-content', defaultMessage: 'Installing content' },
	update_content: { id: 'panel.operation.update-content', defaultMessage: 'Updating content' },
	change_game_version: {
		id: 'panel.operation.change-game-version',
		defaultMessage: 'Changing game version',
	},
	install_java: { id: 'panel.operation.install-java', defaultMessage: 'Installing Java' },
	backup_create: { id: 'panel.operation.backup-create', defaultMessage: 'Backing up' },
	backup_restore: { id: 'panel.operation.backup-restore', defaultMessage: 'Restoring backup' },
	unarchive: { id: 'panel.operation.unarchive', defaultMessage: 'Extracting' },
})

const busyLabels: Record<BusyReasonCode, MessageDescriptor> = {
	installing: messages.installing,
	syncing_content: messages.syncingContent,
	backup_creating: messages.backingUp,
	backup_restoring: messages.restoring,
	deleting: messages.deleting,
}

const servers = ref<Server[]>([])
const owners = ref<Record<Ulid, UserRef>>({})
const loaderNames = ref<Record<string, string>>({})
const operations = ref<Operation[]>([])
const busyReasons = ref<Record<Ulid, BusyReasonCode[]>>({})
const loading = ref(true)
const failure = ref<string | null>(null)
const query = ref('')

let timer: ReturnType<typeof setInterval> | null = null

const capabilities = computed(() => user.value?.capabilities ?? null)

const blocked = computed(() => {
	switch (capabilities.value?.blocked_reason) {
		case 'over_limit':
			return formatMessage(messages.overLimit)
		case 'system_user_not_ready':
			return formatMessage(messages.systemUserNotReady)
		default:
			return null
	}
})

const terms = computed(() =>
	query.value
		.toLowerCase()
		.split(/\s+/)
		.filter((term) => term.length > 0),
)

function haystack(server: Server): string {
	const loader = server.loader ?? ''
	return [
		server.name,
		loader,
		loaderNames.value[loader] ?? '',
		server.game_version ?? '',
		server.game,
		owners.value[server.owner_id]?.username ?? '',
	]
		.join(' ')
		.toLowerCase()
}

const matching = computed(() => {
	if (terms.value.length === 0) return servers.value
	return servers.value.filter((server) => {
		const text = haystack(server)
		return terms.value.every((term) => text.includes(term))
	})
})

interface Row {
	server: Server
	owner: UserRef | undefined
	operation: Operation | undefined
	state: { label: string; color: string }
	address: string
	loaderName: string | null
}

const rows = computed<Row[]>(() =>
	matching.value.map((server) => ({
		server,
		owner: owners.value[server.owner_id],
		operation: operations.value.find((operation) => operation.server_id === server.id),
		state: stateOf(server),
		address: server.net.ip ? `${server.net.ip}:${server.net.port}` : '',
		loaderName: server.loader ? (loaderNames.value[server.loader] ?? server.loader) : null,
	})),
)

const groups = computed(() => {
	const me = user.value?.id
	return [
		{
			key: 'own',
			title: formatMessage(messages.yourServers),
			rows: rows.value.filter((row) => row.server.owner_id === me),
		},
		{
			key: 'shared',
			title: formatMessage(messages.sharedWithYou),
			rows: rows.value.filter((row) => row.server.owner_id !== me),
		},
	].filter((group) => group.rows.length > 0)
})

function stateOf(server: Server): { label: string; color: string } {
	const busy = busyReasons.value[server.id]?.[0]
	if (server.status === 'deleting') return { label: formatMessage(messages.deleting), color: 'red' }
	if (server.status === 'installing' || busy === 'installing') {
		return { label: formatMessage(messages.installing), color: 'orange' }
	}
	if (busy) return { label: formatMessage(busyLabels[busy]), color: 'orange' }
	if (server.status === 'broken') return { label: formatMessage(messages.broken), color: 'red' }
	if (server.flows.intro) return { label: formatMessage(messages.setupPending), color: 'blue' }
	return { label: formatMessage(messages.ready), color: 'green' }
}

function reason(error: unknown): string {
	if (!isApiRequestError(error)) return formatMessage(messages.unreachable)
	return error.message || formatMessage(messages.unreachable)
}

async function load(initial: boolean): Promise<void> {
	if (initial) {
		loading.value = true
		failure.value = null
	}
	try {
		const [list, active] = await Promise.all([
			api.servers.list(),
			api.operations.listAll({ state: 'active' }),
		])
		servers.value = list.servers
		owners.value = list.users
		operations.value = active.operations
		busyReasons.value = active.busy_reasons_by_server
		failure.value = null
		beat()
	} catch (error) {
		if (initial) failure.value = reason(error)
	} finally {
		loading.value = false
	}
}

function beat(): void {
	if (operations.value.length === 0) {
		if (timer !== null) clearInterval(timer)
		timer = null
		return
	}
	timer ??= setInterval(() => void tick(), POLL_INTERVAL_MS)
}

async function tick(): Promise<void> {
	let active
	try {
		active = await api.operations.listAll({ state: 'active' })
	} catch {
		return
	}

	const still = new Set(active.operations.map((operation) => operation.id))
	const ended = operations.value.some((operation) => !still.has(operation.id))
	operations.value = active.operations
	busyReasons.value = active.busy_reasons_by_server

	if (ended) await load(false)
	else beat()
}

async function loadLoaderNames(): Promise<void> {
	const catalog = await api.settings.loaders().catch(() => null)
	if (!catalog) return
	loaderNames.value = Object.fromEntries(catalog.loaders.map((entry) => [entry.id, entry.name]))
}

onMounted(() => {
	void Promise.all([load(true), loadLoaderNames()])
})

onBeforeUnmount(() => {
	if (timer !== null) clearInterval(timer)
	timer = null
})
</script>
