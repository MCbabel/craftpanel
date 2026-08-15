import type { Labrinth } from '@modrinth/api-client'
import { FolderCogIcon } from '@modrinth/assets'
import {
	type BulkOperationStatus,
	type BusyReason,
	type ContentDependencyWarning,
	type ContentItem,
	type ContentManagerContext,
	type ContentModpackData,
	defineMessages,
	type OverflowMenuOption,
	provideContentManager,
	useVIntl,
	type WebNotification,
} from '@modrinth/ui'
import type { ComputedRef, Ref } from 'vue'
import { computed, nextTick, onScopeDispose, ref, watch } from 'vue'
import type { RouteLocationRaw } from 'vue-router'

import {
	api,
	type ApiContentItem,
	type ContentListResponse,
	type ContentModpack,
	type ContentUpdateRequest,
	hasErrorCode,
	type Operation,
	type ServerEventSource,
	type Ulid,
	type UploadProgress,
} from '@/api'
import {
	type ConfigEntry,
	configLocation,
	configPath,
	configRoot,
} from '@/providers/config-location'

export type PanelApi = typeof api
export type Notify = (notification: Partial<WebNotification>) => void

export interface ModrinthVersionSource {
	getProjectVersions(
		id: string,
		options?: { include_changelog?: boolean },
	): Promise<Labrinth.Versions.v2.Version[]>
	getVersion(id: string): Promise<Labrinth.Versions.v2.Version>
	getVersions(ids: string[]): Promise<Labrinth.Versions.v2.Version[]>
	getVersionFromFileHash(hash: string, algorithm: 'sha1'): Promise<Labrinth.Versions.v2.Version>
}

export interface UpdaterModalHandle {
	show: (initialVersionId?: string, options?: { switchMode?: boolean }) => void
	hide: () => void
}

export interface ModpackContentModalHandle {
	show: (items: ContentItem[]) => void
	showLoading: () => void
	hide: () => void
}

export interface FilePickerRequest {
	accept: string
	multiple: boolean
}

const MODRINTH_SITE = 'https://modrinth.com'
const OPERATION_POLL_MS = 5_000
const OPERATION_STALL_MS = 10 * 60 * 1_000

const messages = defineMessages({
	loadFailed: {
		id: 'craftpanel.content.load-failed',
		defaultMessage: 'Failed to load content',
	},
	changeFailed: {
		id: 'craftpanel.content.change-failed',
		defaultMessage: 'Failed to change content',
	},
	partiallyFailed: {
		id: 'craftpanel.content.partially-failed',
		defaultMessage: '{count, number} of {total, number} files could not be changed',
	},
	updateFailed: {
		id: 'craftpanel.content.update-failed',
		defaultMessage: 'Failed to update content',
	},
	uploadFailed: {
		id: 'craftpanel.content.upload-failed',
		defaultMessage: 'Failed to upload files',
	},
	modpackFailed: {
		id: 'craftpanel.content.modpack-failed',
		defaultMessage: 'Failed to change the modpack',
	},
	versionsFailed: {
		id: 'craftpanel.content.versions-failed',
		defaultMessage: 'Failed to load versions from Modrinth',
	},
	noVersionList: {
		id: 'craftpanel.content.no-version-list',
		defaultMessage: 'No versions to choose from',
	},
	withoutProject: {
		id: 'craftpanel.content.without-project',
		defaultMessage:
			'This file is not linked to a Modrinth project, so there is no version list for it. Upload a newer file to replace it.',
	},
	entryGone: {
		id: 'craftpanel.content.entry-gone',
		defaultMessage: 'This entry is no longer on the server.',
	},
	noWritePermission: {
		id: 'craftpanel.content.no-write-permission',
		defaultMessage: 'You do not have permission to change this server’s content',
	},
	configOwn: {
		id: 'craftpanel.content.config-own',
		defaultMessage: 'Open configuration',
	},
	configShared: {
		id: 'craftpanel.content.config-shared',
		defaultMessage: 'Open configuration folder',
	},
	configSharedTooltip: {
		id: 'craftpanel.content.config-shared-tooltip',
		defaultMessage: '{name} has none of its own — this opens {path}',
	},
	configNone: {
		id: 'craftpanel.content.config-none',
		defaultMessage: 'No configuration yet',
	},
	configNoneTooltip: {
		id: 'craftpanel.content.config-none-tooltip',
		defaultMessage:
			'Nothing has been written to {path} yet. A plugin or mod creates its files the first time the server runs.',
	},
})

export interface OperationHost {
	serverId: Ulid
	socket: ServerEventSource
	client: PanelApi
}

export function isAbortError(cause: unknown): boolean {
	return cause instanceof Error && cause.name === 'AbortError'
}

export function awaitOperation(
	host: OperationHost,
	started: Operation,
	onProgress?: (operation: Operation) => void,
	signal?: AbortSignal,
): Promise<Operation> {
	return new Promise((resolve, reject) => {
		let revision = -1
		let progress = started.progress
		let state: string = started.state
		let lastChangeAt = Date.now()
		let settled = false

		const apply = (operation: Operation) => {
			if (settled) return
			if (operation.progress !== progress || operation.state !== state) {
				progress = operation.progress
				state = operation.state
				lastChangeAt = Date.now()
			}
			onProgress?.(operation)
			if (operation.state === 'queued' || operation.state === 'ongoing') return
			settled = true
			stop()
			resolve(operation)
		}

		const unsubscribe = host.socket.on('operations', (message) => {
			if (message.revision <= revision) return
			revision = message.revision
			const operation = message.operations.find((entry) => entry.id === started.id)
			if (operation) apply(operation)
		})

		const timer = setInterval(() => {
			if (settled) return
			if (Date.now() - lastChangeAt > OPERATION_STALL_MS) {
				settled = true
				stop()
				reject(new Error('The server stopped reporting progress.'))
				return
			}
			if (host.socket.status.phase === 'open') return
			host.client.operations
				.get(host.serverId, started.id)
				.then(apply)
				.catch(() => undefined)
		}, OPERATION_POLL_MS)

		const abandon = () => {
			if (settled) return
			settled = true
			stop()
			reject(signal?.reason ?? new DOMException('Aborted', 'AbortError'))
		}
		signal?.addEventListener('abort', abandon)

		function stop() {
			unsubscribe()
			clearInterval(timer)
			signal?.removeEventListener('abort', abandon)
		}

		if (signal?.aborted) {
			abandon()
			return
		}
		apply(started)
	})
}

export interface BusyState {
	isBusy: ComputedRef<boolean>
	busyMessage: ComputedRef<string | null>
}

export function useBusyState(reasons: Ref<BusyReason[]> | ComputedRef<BusyReason[]>): BusyState {
	const { formatMessage } = useVIntl()
	return {
		isBusy: computed(() => reasons.value.length > 0),
		busyMessage: computed(() =>
			reasons.value.length > 0 ? formatMessage(reasons.value[0].reason) : null,
		),
	}
}

export function projectUrl(projectType: string, slugOrId: string): string {
	return `${MODRINTH_SITE}/${projectType}/${encodeURIComponent(slugOrId)}`
}

export function preselectedVersionId(
	target: { has_update: boolean; update_version_id: string | null; version_id: string | null },
	switchMode: boolean,
): string | undefined {
	if (!switchMode && target.has_update && target.update_version_id) return target.update_version_id
	return target.version_id ?? undefined
}

export type UpdaterPlan =
	| { open: true; projectId: string; initialVersionId: string | undefined }
	| { open: false; reason: 'gone' | 'without-project' }

export function updaterPlan(
	source: ApiContentItem | ContentModpack | null | undefined,
	switchMode: boolean,
): UpdaterPlan {
	if (!source) return { open: false, reason: 'gone' }
	if (!source.project_id) return { open: false, reason: 'without-project' }
	return {
		open: true,
		projectId: source.project_id,
		initialVersionId: preselectedVersionId(
			{
				has_update: source.has_update,
				update_version_id: source.update_version_id,
				version_id: 'version_id' in source ? source.version_id : (source.version?.id ?? null),
			},
			switchMode,
		),
	}
}

function toContentItem(item: ApiContentItem): ContentItem {
	const project = item.project
	return {
		id: item.id,
		file_name: item.file_name,
		file_path: item.file_path,
		size: item.size,
		enabled: item.enabled,
		project_type: item.project_type,
		has_update: item.has_update,
		update_version_id: item.update_version_id,
		date_added: item.date_added,
		environment: item.environment ?? undefined,
		pack_client_retained: item.pack_client_retained,
		pack_client_depends: item.pack_client_depends,
		installing: item.installing,
		source_kind: item.source_kind,
		external: item.external,
		external_url: item.external_url ?? undefined,
		project: project
			? {
					id: project.id,
					slug: project.slug ?? project.id,
					title: project.title,
					icon_url: project.icon_url ?? undefined,
				}
			: {
					id: item.project_id ?? item.id,
					slug: item.project_id ?? item.id,
					title: item.file_name,
					icon_url: undefined,
				},
		version: item.version
			? {
					id: item.version.id,
					version_number: item.version.version_number,
					file_name: item.version.file_name,
					date_published: item.version.date_published ?? undefined,
				}
			: undefined,
		owner: item.owner
			? {
					id: item.owner.id,
					name: item.owner.name,
					type: item.owner.type,
					avatar_url: item.owner.avatar_url ?? undefined,
				}
			: undefined,
	}
}

function toModpackData(pack: ContentModpack): ContentModpackData {
	const slugOrId = pack.slug ?? pack.project_id
	const link = slugOrId ? projectUrl('modpack', slugOrId) : undefined
	return {
		project: {
			id: pack.project_id ?? pack.title,
			slug: pack.slug ?? pack.project_id ?? pack.title,
			title: pack.title,
			description: pack.description ?? '',
			icon_url: pack.icon_url ?? undefined,
			downloads: pack.downloads,
			followers: pack.followers,
			filename: pack.filename,
		},
		projectLink: link,
		version:
			pack.version_id && pack.version_number
				? {
						id: pack.version_id,
						version_number: pack.version_number,
						date_published: pack.date_published ?? '',
					}
				: undefined,
		versionLink: link && pack.version_id ? `${link}/version/${pack.version_id}` : undefined,
		owner: pack.owner
			? {
					id: pack.owner.id,
					name: pack.owner.name,
					type: pack.owner.type,
					avatar_url: pack.owner.avatar_url ?? undefined,
					link: `${MODRINTH_SITE}/${pack.owner.type}/${encodeURIComponent(pack.owner.id)}`,
				}
			: undefined,
		categories: pack.categories.map((name) => ({
			name,
			icon: '',
			project_type: 'modpack',
			header: 'categories',
		})),
		hasUpdate: pack.has_update,
	}
}

export function pickFilesFromDocument(request: FilePickerRequest): Promise<File[]> {
	return new Promise((resolve) => {
		const input = document.createElement('input')
		input.type = 'file'
		input.accept = request.accept
		input.multiple = request.multiple
		input.addEventListener('change', () => resolve(input.files ? [...input.files] : []), {
			once: true,
		})
		input.addEventListener('cancel', () => resolve([]), { once: true })
		input.click()
	})
}

async function sha1Hex(file: File): Promise<string> {
	const digest = await crypto.subtle.digest('SHA-1', await file.arrayBuffer())
	return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('')
}

export interface ContentManagerOptions {
	serverId: Ulid
	socket: ServerEventSource
	busyReasons: Ref<BusyReason[]> | ComputedRef<BusyReason[]>
	notify: Notify
	browse: () => void
	modrinth: ModrinthVersionSource
	fileLink: (path: string, editing?: string) => RouteLocationRaw
	openSettings?: () => void
	updaterModal?: Ref<UpdaterModalHandle | null | undefined>
	modpackContentModal?: Ref<ModpackContentModalHandle | null | undefined>
	skipNonEssentialWarnings?: Ref<boolean>
	confirmUnknownFile?: (fileName: string) => Promise<boolean>
	pickFiles?: (request: FilePickerRequest) => Promise<File[]>
	client?: PanelApi
}

export function useContentManager(options: ContentManagerOptions) {
	const { formatMessage } = useVIntl()
	const client = options.client ?? api
	const serverId = options.serverId
	const host: OperationHost = { serverId, socket: options.socket, client }
	const pickFiles = options.pickFiles ?? pickFilesFromDocument
	const { isBusy, busyMessage } = useBusyState(options.busyReasons)

	const lifetime = new AbortController()

	const list = ref<ContentListResponse | null>(null)
	const loading = ref(true)
	const error = ref<Error | null>(null)
	const isBulkOperating = ref(false)
	const uploadProgress = ref<UploadProgress | null>(null)
	let cancelUpload: (() => void) | null = null
	let reloadPending = false

	const items = computed(() => (list.value?.items ?? []).map(toContentItem))
	const lockedIds = computed(
		() => new Set((list.value?.items ?? []).filter((item) => item.locked).map((item) => item.id)),
	)
	const modrinthIds = computed(
		() =>
			new Set((list.value?.items ?? []).filter((item) => item.project_id).map((item) => item.id)),
	)
	const canWrite = computed(() => list.value?.permissions.can_write ?? false)
	const contentTypeLabel = computed(() => list.value?.content_type ?? 'mod')
	const currentGameVersion = computed(() => list.value?.game_version ?? '')
	const currentLoader = computed(() => list.value?.loader ?? '')
	const modpack = computed(() => (list.value?.modpack ? toModpackData(list.value.modpack) : null))
	const truncated = computed(() => list.value?.truncated ?? false)

	function reportError(descriptor: (typeof messages)[keyof typeof messages], cause: unknown) {
		if (lifetime.signal.aborted || isAbortError(cause)) return
		options.notify({
			type: 'error',
			title: formatMessage(descriptor),
			text: cause instanceof Error ? cause.message : undefined,
		})
	}

	async function load(refreshUpdates = false) {
		if (lifetime.signal.aborted) return
		try {
			list.value = await client.content.list(
				serverId,
				{ refresh_updates: refreshUpdates },
				{ signal: lifetime.signal },
			)
			error.value = null
			void readConfigRoots(list.value)
		} catch (cause) {
			if (isAbortError(cause)) throw cause
			error.value = cause instanceof Error ? cause : new Error(String(cause))
			throw cause
		} finally {
			loading.value = false
		}
	}

	async function reload(refreshUpdates = false) {
		if (isBulkOperating.value) {
			reloadPending = true
			return
		}
		await load(refreshUpdates).catch(() => undefined)
	}

	const configListings = ref(new Map<string, ConfigEntry[]>())

	async function readConfigRoots(fetched: ContentListResponse) {
		const roots = new Set<string>()
		for (const item of fetched.items) {
			const root = configRoot(fetched.content_type, item.file_path)
			if (root !== null) roots.add(root)
		}

		const read = await Promise.all(
			[...roots].map(async (root) => {
				try {
					const page = await client.files.listAll(serverId, root, { signal: lifetime.signal })
					return [root, page.items as ConfigEntry[]] as const
				} catch (cause) {
					return [root, hasErrorCode(cause, 'not_found') ? [] : null] as const
				}
			}),
		)
		if (lifetime.signal.aborted) return
		configListings.value = new Map(
			read.filter((pair): pair is readonly [string, ConfigEntry[]] => pair[1] !== null),
		)
	}

	function configOptions(itemId: string): OverflowMenuOption[] {
		const fetched = list.value
		const row = fetched?.items.find((entry) => entry.id === itemId)
		if (!fetched || !row) return []

		const spot = configLocation(
			{
				contentType: fetched.content_type,
				filePath: row.file_path,
				fileName: row.file_name,
				title: row.project?.title ?? null,
				slug: row.project?.slug ?? null,
			},
			configListings.value,
		)
		if (spot === null) return []

		if (spot.kind === 'none') {
			return [
				{
					id: 'configuration',
					label: formatMessage(messages.configNone),
					icon: FolderCogIcon,
					disabled: true,
					tooltip: formatMessage(messages.configNoneTooltip, { path: spot.path }),
					action: () => undefined,
				},
			]
		}

		return [
			{
				id: 'configuration',
				type: 'link',
				label: formatMessage(spot.kind === 'own' ? messages.configOwn : messages.configShared),
				icon: FolderCogIcon,
				tooltip:
					spot.kind === 'own'
						? configPath(spot)
						: formatMessage(messages.configSharedTooltip, {
								name: row.project?.title ?? row.file_name,
								path: spot.path,
							}),
				to: options.fileLink(spot.path, spot.kind === 'own' ? spot.editing : undefined),
			},
		]
	}

	async function mutate(action: 'enable' | 'disable' | 'delete', targets: ContentItem[]) {
		const ids = targets.filter((item) => !lockedIds.value.has(item.id)).map((item) => item.id)
		if (ids.length === 0) return

		const endpoint =
			action === 'enable'
				? client.content.enable
				: action === 'disable'
					? client.content.disable
					: client.content.remove

		try {
			const response = await endpoint(serverId, { ids })
			const failed = response.results.filter((result) => !result.ok)
			if (failed.length === 0) return
			if (failed.length === response.results.length) {
				throw new Error(failed[0].message ?? formatMessage(messages.changeFailed))
			}
			options.notify({
				type: 'warning',
				title: formatMessage(messages.changeFailed),
				text: formatMessage(messages.partiallyFailed, {
					count: failed.length,
					total: response.results.length,
				}),
			})
		} catch (cause) {
			reportError(messages.changeFailed, cause)
			throw cause
		} finally {
			await reload()
		}
	}

	async function setEnabled(targets: ContentItem[], enabled: boolean) {
		await mutate(enabled ? 'enable' : 'disable', targets)
	}

	async function runUpdate(
		request: ContentUpdateRequest,
		onProgress?: (status: BulkOperationStatus) => void,
	) {
		try {
			const accepted = await client.content.update(serverId, request)
			const total = accepted.total
			const finished = await awaitOperation(
				host,
				accepted.operation,
				(operation) => {
					onProgress?.({
						total,
						progress: Math.min(total, Math.round(operation.progress * total)),
						waiting: operation.state === 'queued',
					})
				},
				lifetime.signal,
			)
			if (finished.state === 'failed') {
				options.notify({
					type: 'error',
					title: formatMessage(messages.updateFailed),
					text: finished.error?.message,
				})
			}
		} catch (cause) {
			reportError(messages.updateFailed, cause)
		} finally {
			await load(false).catch(() => undefined)
		}
	}

	const updaterItem = ref<ContentItem | null>(null)
	const updaterModpack = ref(false)
	const updaterSwitchMode = ref(false)
	const updaterVersions = ref<Labrinth.Versions.v2.Version[]>([])
	const updaterLoading = ref(false)
	const updaterLoadingChangelog = ref(false)

	const updaterVisible = computed(() => updaterItem.value !== null || updaterModpack.value)
	const updaterCurrentVersionId = computed(() =>
		updaterModpack.value
			? (list.value?.modpack?.version_id ?? '')
			: (updaterItem.value?.version?.id ?? ''),
	)

	function closeUpdater() {
		updaterItem.value = null
		updaterModpack.value = false
		updaterSwitchMode.value = false
		updaterVersions.value = []
		updaterLoading.value = false
		updaterLoadingChangelog.value = false
	}

	async function openUpdater(itemId: Ulid | null, switchMode: boolean) {
		if (isBusy.value) return
		const isModpack = itemId === null

		const source = isModpack
			? (list.value?.modpack ?? null)
			: (list.value?.items ?? []).find((entry) => entry.id === itemId)
		const plan = updaterPlan(source, switchMode)
		if (!plan.open) {
			options.notify({
				type: 'warning',
				title: formatMessage(messages.noVersionList),
				text: formatMessage(
					plan.reason === 'gone' ? messages.entryGone : messages.withoutProject,
				),
			})
			return
		}

		updaterItem.value = isModpack
			? null
			: (items.value.find((entry) => entry.id === itemId) ?? null)
		updaterModpack.value = isModpack
		updaterSwitchMode.value = switchMode
		updaterVersions.value = []
		updaterLoading.value = true

		await nextTick()
		options.updaterModal?.value?.show(plan.initialVersionId, { switchMode })
		try {
			const versions = await options.modrinth.getProjectVersions(plan.projectId, {
				include_changelog: false,
			})
			updaterVersions.value = [...versions].sort(
				(left, right) =>
					new Date(right.date_published).getTime() - new Date(left.date_published).getTime(),
			)
		} catch (cause) {
			reportError(messages.versionsFailed, cause)
		} finally {
			updaterLoading.value = false
		}
	}

	async function loadChangelog(version: Labrinth.Versions.v2.Version) {
		if (version.changelog) return
		updaterLoadingChangelog.value = true
		try {
			const full = await options.modrinth.getVersion(version.id)
			updaterVersions.value = updaterVersions.value.map((entry) =>
				entry.id === full.id ? full : entry,
			)
		} catch {
		} finally {
			updaterLoadingChangelog.value = false
		}
	}

	async function confirmUpdaterVersion(version: Labrinth.Versions.v2.Version) {
		const target = updaterItem.value
		const isModpack = updaterModpack.value
		options.updaterModal?.value?.hide()
		closeUpdater()
		if (isModpack) {
			await runModpackOperation(() =>
				client.content.updateModpack(serverId, { version_id: version.id }),
			)
			return
		}
		if (!target) return
		await runUpdate({ items: [{ id: target.id, version_id: version.id }], all: false })
	}

	async function runModpackOperation(start: () => Promise<{ operation: Operation }>) {
		try {
			const accepted = await start()
			const finished = await awaitOperation(host, accepted.operation, undefined, lifetime.signal)
			if (finished.state === 'failed') {
				options.notify({
					type: 'error',
					title: formatMessage(messages.modpackFailed),
					text: finished.error?.message,
				})
			}
		} catch (cause) {
			reportError(messages.modpackFailed, cause)
		} finally {
			await load(false).catch(() => undefined)
		}
	}

	async function viewModpackContent() {
		const modal = options.modpackContentModal?.value
		modal?.showLoading()
		try {
			const response = await client.content.modpackContents(serverId)
			modal?.show(response.items.map(toContentItem))
		} catch (cause) {
			modal?.hide()
			reportError(messages.loadFailed, cause)
		}
	}

	async function unlinkModpack() {
		try {
			await client.content.unlinkModpack(serverId)
		} catch (cause) {
			reportError(messages.modpackFailed, cause)
		} finally {
			await reload()
		}
	}

	async function isKnownToModrinth(file: File): Promise<boolean> {
		try {
			await options.modrinth.getVersionFromFileHash(await sha1Hex(file), 'sha1')
			return true
		} catch {
			return false
		}
	}

	async function uploadFiles() {
		const picked = await pickFiles({ accept: '.jar,.zip', multiple: true })
		if (picked.length === 0) return

		const confirm = options.confirmUnknownFile
		const selection: File[] = []
		for (const file of picked) {
			if (!confirm || (await isKnownToModrinth(file)) || (await confirm(file.name))) {
				selection.push(file)
			}
		}
		if (selection.length === 0) return

		const handle = client.content.upload(serverId, selection)
		handle.onProgress((progress) => {
			uploadProgress.value = progress
		})
		cancelUpload = handle.cancel
		try {
			const response = await handle.promise
			const failed = response.results.filter((result) => !result.ok)
			if (failed.length > 0) {
				options.notify({
					type: 'warning',
					title: formatMessage(messages.uploadFailed),
					text: failed.map((result) => `${result.file_name}: ${result.message ?? ''}`).join('\n'),
				})
			}
		} catch (cause) {
			reportError(messages.uploadFailed, cause)
		} finally {
			uploadProgress.value = null
			cancelUpload = null
			await reload()
		}
	}

	async function getDeleteDependencyWarning(
		targets: ContentItem[],
	): Promise<ContentDependencyWarning | null> {
		if (targets.length === 0) return null
		const response = await client.content.dependents(serverId, {
			ids: targets.map((item) => item.id),
		})
		const byId = new Map(items.value.map((item) => [item.id, item]))
		const dependents = response.dependents.flatMap((entry) => {
			const item = byId.get(entry.id)
			if (!item) return []
			const dependencies = entry.depends_on
				.map((id) => byId.get(id))
				.filter((value): value is ContentItem => value !== undefined)
			return dependencies.length > 0 ? [{ item, dependencies }] : []
		})
		if (dependents.length === 0) return null
		const needed = new Set(dependents.flatMap((entry) => entry.dependencies.map((item) => item.id)))
		return { items: targets.filter((item) => needed.has(item.id)), dependents }
	}

	const stopContentChanged = options.socket.on('content_changed', () => {
		void reload()
	})

	let powerState: string | null = null
	const stopState = options.socket.on('state', (message) => {
		const seen = powerState
		powerState = message.power_state
		if (seen !== null && seen !== powerState && list.value !== null) {
			void readConfigRoots(list.value)
		}
	})

	watch(isBulkOperating, (running) => {
		if (running || !reloadPending) return
		reloadPending = false
		void reload()
	})

	onScopeDispose(() => {
		stopContentChanged()
		stopState()
		lifetime.abort()
	})

	const context: ContentManagerContext = {
		items,
		loading,
		error,
		modpack,
		isPackLocked: ref(false),
		isBusy,
		busyMessage,
		skipNonEssentialWarnings: options.skipNonEssentialWarnings,
		disableAddContent: computed(() => !canWrite.value),
		disableAddContentTooltip: formatMessage(messages.noWritePermission),
		contentTypeLabel,
		toggleEnabled: (item) => setEnabled([item], !item.enabled),
		deleteItem: (item) => mutate('delete', [item]),
		refresh: () => reload(true),
		browse: options.browse,
		uploadFiles: () => void uploadFiles(),
		bulkDeleteItems: (targets) => mutate('delete', targets),
		bulkEnableItems: (targets) => setEnabled(targets, true),
		bulkDisableItems: (targets) => setEnabled(targets, false),
		canDeleteItem: (item) => canWrite.value && !lockedIds.value.has(item.id),
		canToggleItem: (item) => canWrite.value && !lockedIds.value.has(item.id),
		getDeleteDependencyWarning,
		hasUpdateSupport: true,
		updateItem: (id) => void openUpdater(id, false),
		bulkUpdateAll: (onProgress) => runUpdate({ items: [], all: true }, onProgress),
		bulkUpdateItems: (targets) =>
			runUpdate({
				items: targets.map((item) => ({ id: item.id, version_id: null })),
				all: false,
			}),
		updateModpack: () => void openUpdater(null, false),
		viewModpackContent: () => void viewModpackContent(),
		unlinkModpack: () => void unlinkModpack(),
		openSettings: options.openSettings,
		switchVersion: (item) => void openUpdater(item.id, true),
		getOverflowOptions: (item) => configOptions(item.id),
		isBulkOperating,
		deletionContext: 'server',
		getItemId: (item) => item.id,
		mapToTableItem: (item) => {
			const modrinthProject = modrinthIds.value.has(item.id)
			const link = modrinthProject ? projectUrl(item.project_type, item.project.slug) : undefined
			return {
				id: item.id,
				project: item.project,
				projectLink: link,
				version: item.version,
				versionLink: link && item.version ? `${link}/version/${item.version.id}` : undefined,
				owner: item.owner,
				enabled: item.enabled,
				installing: item.installing,
				hasUpdate: item.has_update,
				hideDelete: !canWrite.value || lockedIds.value.has(item.id),
				hideSwitchVersion: !canWrite.value || !modrinthProject || !item.version,
			}
		},
		filterPersistKey: `server:${serverId}`,
	}

	provideContentManager(context)
	void load(false).catch(() => undefined)

	return {
		context,
		truncated,
		currentGameVersion,
		currentLoader,
		uploadProgress,
		cancelUpload: () => cancelUpload?.(),
		reload,
		setEnabled,
		updater: {
			visible: updaterVisible,
			isModpack: updaterModpack,
			switchMode: updaterSwitchMode,
			item: updaterItem,
			versions: updaterVersions,
			loading: updaterLoading,
			loadingChangelog: updaterLoadingChangelog,
			currentVersionId: updaterCurrentVersionId,
			projectType: computed(() =>
				updaterModpack.value ? 'modpack' : (updaterItem.value?.project_type ?? undefined),
			),
			projectName: computed(() =>
				updaterModpack.value
					? (list.value?.modpack?.title ?? '')
					: (updaterItem.value?.project.title ?? updaterItem.value?.file_name ?? ''),
			),
			projectIconUrl: computed(() =>
				updaterModpack.value
					? (list.value?.modpack?.icon_url ?? undefined)
					: updaterItem.value?.project.icon_url,
			),
			confirm: (version: Labrinth.Versions.v2.Version) => confirmUpdaterVersion(version),
			cancel: closeUpdater,
			select: loadChangelog,
			hover: loadChangelog,
		},
	}
}
