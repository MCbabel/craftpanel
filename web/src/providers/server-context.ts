import type { Archon, UploadState } from '@modrinth/api-client'
import { defineMessages, type MessageDescriptor } from '@modrinth/ui/src/composables/i18n.ts'
import type { FileOperation } from '@modrinth/ui/src/layouts/shared/files-tab/types.ts'
import {
	type BusyReason,
	type CancelUploadHandler,
	type FilesystemAuth,
	type ModrinthServerContext,
	provideModrinthServerContext,
	type ServerStats,
	type ServerStatsSample,
} from '@modrinth/ui/src/providers/server-context.ts'
import { computed, type ComputedRef, onScopeDispose, ref, type Ref } from 'vue'

import { api } from '@/api'
import type {
	BusyReasonCode,
	LoaderId,
	Operation,
	OperationKind,
	OperationListResponse,
	OperationPhase,
	PowerState,
	PowerTarget,
	Server,
	ServerEventSource,
	ServerSocketStatus,
	ServerStatus,
	Ulid,
	WsStateMessage,
	WsStatsMessage,
} from '@/api'

export const WORLD_ID = 'default'

const POWER_STATES: Record<PowerState, Archon.Websocket.v0.PowerState> = {
	stopped: 'stopped',
	starting: 'starting',
	running: 'running',
	stopping: 'stopping',
	crashed: 'crashed',
}

const UNKNOWN_BUSY_MESSAGE: MessageDescriptor = {
	id: 'servers.admonitions.background-task-running',
	defaultMessage: 'Background task running',
}

const BUSY_MESSAGES: Record<BusyReasonCode, MessageDescriptor> = defineMessages({
	installing: {
		id: 'servers.busy.installing',
		defaultMessage: 'Server is installing',
	},
	syncing_content: {
		id: 'servers.busy.syncing-content',
		defaultMessage: 'Content sync in progress',
	},
	backup_creating: {
		id: 'servers.busy.backup-creating',
		defaultMessage: 'Backup creation in progress',
	},
	backup_restoring: {
		id: 'servers.busy.backup-restoring',
		defaultMessage: 'Backup restore in progress',
	},
	deleting: {
		id: 'servers.busy.deleting',
		defaultMessage: 'Server is being deleted',
	},
})

const INSTALL_PHASES: Record<OperationPhase, Archon.Websocket.v0.SyncInstallPhase> = {
	analyzing: 'Analyzing',
	installing_loader: 'InstallingLoader',
	verifying: 'InstallingLoader',
	running_installer: 'InstallingLoader',
	installing_java: 'InstallingLoader',
	installing_pack: 'InstallingPack',
	addons: 'Addons',
	writing_config: 'Addons',
}

const BANNER_KINDS = new Set<OperationKind>([
	'server_create',
	'install_loader',
	'repair_content',
	'reset_server',
	'install_modpack',
	'change_game_version',
	'install_java',
	'install_content',
	'update_content',
])

const LOADER_NAMES: Record<LoaderId, string> = {
	vanilla: 'Vanilla',
	paper: 'Paper',
	folia: 'Folia',
	purpur: 'Purpur',
	leaf: 'Leaf',
	fabric: 'Fabric',
	velocity: 'Velocity',
	neoforge: 'NeoForge',
	quilt: 'Quilt',
	forge: 'Forge',
}

const GRAPH_POINTS = 10
const STALE_STATS_AFTER_MS = 5_000
const STALE_STATS_EVERY_MS = 1_000
const MIB = 1024 * 1024

export interface InstallProgress {
	phase: Archon.Websocket.v0.SyncInstallPhase
	percent: number
}

export interface InstallError {
	step: string
	description: string
}

export type ServerContextClient = Pick<typeof api, 'operations' | 'servers'>

export interface ServerContextInput {
	server: Server
	socket: ServerEventSource
	client?: ServerContextClient
	loaderName?: (loader: LoaderId) => string | undefined
}

export interface ServerContextHandle {
	context: ModrinthServerContext
	server: Ref<Server>
	socketStatus: Ref<ServerSocketStatus>
	operations: Ref<Operation[]>
	busyReasonCodes: Ref<BusyReasonCode[]>
	powerTarget: Ref<PowerTarget | null>
	installOperation: ComputedRef<Operation | null>
	installProgress: ComputedRef<InstallProgress | null>
	installError: ComputedRef<InstallError | null>
	refreshServer: () => Promise<void>
	refreshOperations: () => Promise<void>
}

function busyReasonOf(code: BusyReasonCode): MessageDescriptor {
	return BUSY_MESSAGES[code] ?? UNKNOWN_BUSY_MESSAGE
}

function emptySample(ramTotalBytes: number): ServerStatsSample {
	return {
		cpu_percent: 0,
		ram_usage_bytes: 0,
		ram_total_bytes: ramTotalBytes,
		storage_usage_bytes: 0,
		storage_total_bytes: 0,
	}
}

function ramPercent(sample: ServerStatsSample): number {
	if (sample.ram_total_bytes <= 0) return 0
	return Math.floor((sample.ram_usage_bytes / sample.ram_total_bytes) * 100)
}

function appendPoint(points: number[], value: number): number[] {
	const next = [...points, value]
	return next.length > GRAPH_POINTS ? next.slice(next.length - GRAPH_POINTS) : next
}

function vendorStatus(status: ServerStatus): Archon.Servers.v0.Status {
	return status === 'deleting' ? ('deleting' as Archon.Servers.v0.Status) : status
}

function toFileOperation(operation: Operation): FileOperation {
	return {
		id: operation.id,
		op: operation.kind,
		src: operation.src ?? '',
		state: operation.state,
		progress: operation.progress,
		bytes_processed: operation.bytes_processed ?? undefined,
		files_processed: operation.files_processed ?? undefined,
		current_file: operation.current_file ?? undefined,
	}
}

export function provideServerContext(input: ServerContextInput): ServerContextHandle {
	const { socket } = input
	const client = input.client ?? api
	const serverId: Ulid = input.server.id
	const server = ref<Server>(input.server)

	const socketStatus = ref<ServerSocketStatus>(socket.status)
	const isConnected = ref(socket.status.phase === 'open')
	const isWsAuthIncorrect = ref(socket.status.authIncorrect)

	const powerState = ref<Archon.Websocket.v0.PowerState>('stopped')
	const powerStateDetails = ref<{ oom_killed?: boolean; exit_code?: number }>()
	const powerTarget = ref<PowerTarget | null>(null)
	const uptimeSeconds = ref(0)

	const stats = ref<ServerStats>({
		current: emptySample(input.server.memory_mib * MIB),
		past: emptySample(input.server.memory_mib * MIB),
		graph: { cpu: [], ram: [] },
	})

	const operations = ref<Operation[]>([])
	const busyReasonCodes = ref<BusyReasonCode[]>([])
	const dismissedOperationIds = ref(new Set<Ulid>())
	let revision = -1

	const reads = new AbortController()

	function fail(what: string, error: unknown): void {
		if (reads.signal.aborted) return
		console.error(`[server-context] ${what} failed`, error)
	}

	let uptimeTicker: ReturnType<typeof setInterval> | null = null
	let staleStatsTimeout: ReturnType<typeof setTimeout> | null = null
	let staleStatsTicker: ReturnType<typeof setInterval> | null = null

	function stopUptimeTicker(): void {
		if (uptimeTicker === null) return
		clearInterval(uptimeTicker)
		uptimeTicker = null
	}

	function stopStaleStatsWatchdog(): void {
		if (staleStatsTimeout !== null) clearTimeout(staleStatsTimeout)
		if (staleStatsTicker !== null) clearInterval(staleStatsTicker)
		staleStatsTimeout = null
		staleStatsTicker = null
	}

	function pushSample(sample: ServerStatsSample): void {
		stats.value = {
			current: sample,
			past: stats.value.current,
			graph: {
				cpu: appendPoint(stats.value.graph.cpu, sample.cpu_percent),
				ram: appendPoint(stats.value.graph.ram, ramPercent(sample)),
			},
		}
	}

	function pushIdleSample(): void {
		pushSample({ ...stats.value.current, cpu_percent: 0, ram_usage_bytes: 0 })
	}

	function armStaleStatsWatchdog(): void {
		stopStaleStatsWatchdog()
		staleStatsTimeout = setTimeout(() => {
			staleStatsTimeout = null
			pushIdleSample()
			staleStatsTicker = setInterval(pushIdleSample, STALE_STATS_EVERY_MS)
		}, STALE_STATS_AFTER_MS)
	}

	function applyState(message: WsStateMessage): void {
		powerState.value = POWER_STATES[message.power_state]
		powerStateDetails.value =
			message.power_state === 'crashed'
				? { oom_killed: message.oom_killed, exit_code: message.exit_code ?? undefined }
				: undefined
		powerTarget.value = message.target

		stopUptimeTicker()
		if (message.power_state === 'stopped' || message.power_state === 'crashed') {
			uptimeSeconds.value = 0
			return
		}
		uptimeSeconds.value = message.uptime_seconds
		uptimeTicker = setInterval(() => {
			uptimeSeconds.value += 1
		}, 1000)
	}

	function applyStats(message: WsStatsMessage): void {
		armStaleStatsWatchdog()
		pushSample({
			cpu_percent: message.cpu_percent,
			ram_usage_bytes: message.ram_usage_bytes,
			ram_total_bytes: message.ram_total_bytes,
			storage_usage_bytes: message.storage_usage_bytes,
			storage_total_bytes: message.storage_total_bytes,
		})
	}

	function applyOperations(snapshot: OperationListResponse): void {
		if (snapshot.revision <= revision) return
		revision = snapshot.revision
		operations.value = snapshot.operations
		busyReasonCodes.value = snapshot.busy_reasons

		const known = new Set(snapshot.operations.map((operation) => operation.id))
		for (const id of dismissedOperationIds.value) {
			if (!known.has(id)) dismissedOperationIds.value.delete(id)
		}
	}

	function applySocketStatus(status: ServerSocketStatus): void {
		if (status.phase === 'open' && socketStatus.value.phase !== 'open') revision = -1
		socketStatus.value = status
		isConnected.value = status.phase === 'open'
		isWsAuthIncorrect.value = status.authIncorrect
		if (!isConnected.value) {
			stopUptimeTicker()
			stopStaleStatsWatchdog()
		}
	}

	async function refreshServer(): Promise<void> {
		server.value = await client.servers.get(serverId, { signal: reads.signal })
	}

	async function refreshOperations(): Promise<void> {
		applyOperations(
			await client.operations.list(serverId, { state: 'all' }, { signal: reads.signal }),
		)
	}

	let serverRefreshRunning = false
	let serverRefreshPending = false

	async function refreshServerQueued(): Promise<void> {
		if (serverRefreshRunning) {
			serverRefreshPending = true
			return
		}
		serverRefreshRunning = true
		try {
			do {
				serverRefreshPending = false
				await refreshServer()
			} while (serverRefreshPending)
		} finally {
			serverRefreshRunning = false
		}
	}

	const unsubscribe = [
		socket.on('status', applySocketStatus),
		socket.on('server', (message) => {
			server.value = message.server
		}),
		socket.on('state', applyState),
		socket.on('stats', applyStats),
		socket.on('operations', applyOperations),
		socket.on('backup_list_changed', () => {
			refreshServerQueued().catch((error: unknown) => fail('refreshing the server', error))
		}),
	]

	onScopeDispose(() => {
		for (const off of unsubscribe) off()
		stopUptimeTicker()
		stopStaleStatsWatchdog()
		reads.abort()
	})

	const vendorServer = computed<Archon.Servers.v0.Server>(() => {
		const current = server.value
		const loaderName =
			current.loader === null
				? null
				: (input.loaderName?.(current.loader) ?? LOADER_NAMES[current.loader])
		const status = busyReasonCodes.value.includes('installing') ? 'installing' : current.status

		return {
			server_id: current.id,
			name: current.name,
			owner_id: current.owner_id,
			net: current.net,
			game: current.game,
			backup_quota: current.backup_quota,
			used_backup_quota: current.used_backup_quota,
			status: vendorStatus(status),
			suspension_reason: null,
			loader: loaderName as Archon.Servers.v0.Loader | null,
			loader_version: current.loader_version,
			mc_version: current.game_version,
			upstream: current.upstream,
			sftp_username: '',
			sftp_password: '',
			sftp_host: '',
			datacenter: '',
			notices: [],
			node: null,
			flows: current.flows,
			is_medal: false,
			current_user_permissions: current.current_user_permissions as unknown as number,
		}
	})

	const installOperation = computed<Operation | null>(() => {
		const running = operations.value.find(
			(operation) =>
				BANNER_KINDS.has(operation.kind) &&
				(operation.state === 'queued' || operation.state === 'ongoing'),
		)
		if (running !== undefined) return running
		const failed = operations.value.find(
			(operation) =>
				BANNER_KINDS.has(operation.kind) &&
				operation.state === 'failed' &&
				operation.error !== null,
		)
		return failed ?? null
	})

	const installProgress = computed<InstallProgress | null>(() => {
		const operation = installOperation.value
		if (operation === null || operation.phase === null || operation.state === 'failed') return null
		return { phase: INSTALL_PHASES[operation.phase], percent: operation.progress * 100 }
	})

	const installError = computed<InstallError | null>(() => {
		const operation = installOperation.value
		if (operation?.state !== 'failed' || operation.error === null) return null
		return { step: operation.error.step, description: operation.error.message }
	})

	async function dismissOperation(opId: string, action: 'dismiss' | 'cancel'): Promise<void> {
		if (action === 'dismiss') dismissedOperationIds.value.add(opId)
		try {
			if (action === 'dismiss') await client.operations.dismiss(serverId, opId)
			else await client.operations.cancel(serverId, opId)
		} catch (error) {
			dismissedOperationIds.value.delete(opId)
			fail(`${action} of operation ${opId}`, error)
		}
	}

	const context: ModrinthServerContext = {
		serverId,
		worldId: ref<string | null>(WORLD_ID),
		server: vendorServer,
		serverFull: computed<Archon.Servers.v1.ServerFull | null>(() => null),
		currentUserPermissions: computed(() => vendorServer.value.current_user_permissions),

		isConnected,
		isWsAuthIncorrect,
		powerState,
		powerStateDetails,
		isServerRunning: computed(() => powerState.value === 'running'),
		stats,
		uptimeSeconds,

		isSyncingContent: computed(() => busyReasonCodes.value.includes('syncing_content')),
		busyReasons: computed<BusyReason[]>(() =>
			busyReasonCodes.value.map((code) => ({ reason: busyReasonOf(code) })),
		),

		fsAuth: ref<FilesystemAuth | null>(null),
		fsOps: ref<Archon.Websocket.v0.FilesystemOperation[]>([]),
		fsQueuedOps: ref<Archon.Websocket.v0.QueuedFilesystemOp[]>([]),
		refreshFsAuth: () => Promise.resolve(),

		uploadState: ref<UploadState>({
			isUploading: false,
			currentFileName: null,
			currentFileProgress: 0,
			uploadedBytes: 0,
			totalBytes: 0,
			completedFiles: 0,
			totalFiles: 0,
		}),
		cancelUpload: ref<CancelUploadHandler | null>(null),

		activeOperations: computed<FileOperation[]>(() =>
			operations.value
				.filter(
					(operation) =>
						operation.kind === 'unarchive' &&
						operation.state !== 'cancelled' &&
						!dismissedOperationIds.value.has(operation.id),
				)
				.map(toFileOperation),
		),
		dismissOperation,
	}

	provideModrinthServerContext(context)
	refreshOperations().catch((error: unknown) => fail('loading the operations', error))

	return {
		context,
		server,
		socketStatus,
		operations,
		busyReasonCodes,
		powerTarget,
		installOperation,
		installProgress,
		installError,
		refreshServer,
		refreshOperations,
	}
}
