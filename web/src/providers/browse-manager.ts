import type { AbstractModrinthClient, Labrinth, RequestOptions } from '@modrinth/api-client'
import { CheckIcon, DownloadIcon, SpinnerIcon } from '@modrinth/assets'
import {
	type BrowseInstallContext,
	type BrowseManagerContext,
	type BrowseSearchResponse,
	type BrowseSelectedProject,
	type BusyReason,
	type CardAction,
	commonMessages,
	defineMessages,
	type EnvironmentSearchOverride,
	type FilterValue,
	injectModrinthClient,
	injectNotificationManager,
	type MessageDescriptor,
	provideBrowseManager,
	type Tags,
	useBrowseSearch,
	useVIntl,
} from '@modrinth/ui'
import { computed, type ComputedRef, onScopeDispose, ref, type Ref, watch } from 'vue'
import { type RouteLocationRaw, useRoute, useRouter } from 'vue-router'

import {
	api,
	type ContentListResponse,
	type ContentSkippedEntry,
	type ContentSkipReason,
	type ServerEventSource,
	type Ulid,
} from '@/api'
import { isAbortError, type PanelApi, projectUrl, useBusyState } from '@/providers/content-manager'
import {
	installedFacts,
	installedProjects,
	installState,
	isSelectable,
	stillSelectable,
} from '@/providers/installed-projects'
import { loaderFacet } from '@/providers/loader-facets'

export type DisplayMode = 'list' | 'grid' | 'gallery'

const DISPLAY_MODE_KEY = 'craftpanel.browse.display-mode'
const DISPLAY_MODES: DisplayMode[] = ['list', 'grid', 'gallery']

const MAX_RESULTS: Record<DisplayMode, number[]> = {
	list: [5, 10, 15, 20, 50, 100],
	grid: [6, 12, 18, 24, 48, 96],
	gallery: [6, 10, 16, 20, 50, 100],
}

const messages = defineMessages({
	back: {
		id: 'craftpanel.browse.back',
		defaultMessage: 'Back to content',
	},
	installFailed: {
		id: 'craftpanel.browse.install-failed',
		defaultMessage: 'The installation could not be started',
	},
	installStarted: {
		id: 'craftpanel.browse.install-started',
		defaultMessage: '{count, plural, one {Installing # project} other {Installing # projects}}',
	},
	installStartedText: {
		id: 'craftpanel.browse.install-started-text',
		defaultMessage: 'Progress is shown above the content page.',
	},
	skippedTitle: {
		id: 'craftpanel.browse.skipped-title',
		defaultMessage: 'Not everything could be installed',
	},
	skippedEntry: {
		id: 'craftpanel.browse.skipped-entry',
		defaultMessage: '{name}: {reason}',
	},
	noWritePermission: {
		id: 'craftpanel.browse.no-write-permission',
		defaultMessage: 'You do not have permission to install content on this server',
	},
	lockedGameVersion: {
		id: 'craftpanel.browse.locked.game-version',
		defaultMessage: 'The game version comes from this server',
	},
	lockedLoader: {
		id: 'craftpanel.browse.locked.loader',
		defaultMessage: 'The loader comes from this server',
	},
	lockedProvidedBy: {
		id: 'craftpanel.browse.locked.provided-by',
		defaultMessage: 'Set by this server',
	},
	lockedSync: {
		id: 'craftpanel.browse.locked.sync',
		defaultMessage: 'Match the server again',
	},
})

const SKIP_REASONS: Record<ContentSkipReason, MessageDescriptor> = defineMessages({
	already_installed: {
		id: 'craftpanel.browse.skip.already-installed',
		defaultMessage: 'already installed',
	},
	duplicate_project: {
		id: 'craftpanel.browse.skip.duplicate-project',
		defaultMessage: 'selected twice',
	},
	conflicting_dependency: {
		id: 'craftpanel.browse.skip.conflicting-dependency',
		defaultMessage: 'two versions wanted at once',
	},
	no_compatible_version: {
		id: 'craftpanel.browse.skip.no-compatible-version',
		defaultMessage: 'no version for this game version and loader',
	},
	missing_version: {
		id: 'craftpanel.browse.skip.missing-version',
		defaultMessage: 'the requested version is gone',
	},
	quilt_fabric_api: {
		id: 'craftpanel.browse.skip.quilt-fabric-api',
		defaultMessage: 'Quilt brings Fabric API itself',
	},
})

async function readTags(client: AbstractModrinthClient, signal: AbortSignal): Promise<Tags> {
	const request: RequestOptions = { api: 'labrinth', version: 2, method: 'GET', signal }
	const [gameVersions, loaders, categories] = await Promise.all([
		client.request<Labrinth.Tags.v2.GameVersion[]>('/tag/game_version', request),
		client.request<Labrinth.Tags.v2.Loader[]>('/tag/loader', request),
		client.request<Labrinth.Tags.v2.Category[]>('/tag/category', request),
	])
	return { gameVersions, loaders, categories }
}

function searchParams(requestParams: string): Labrinth.Search.SearchParams {
	const params = new URLSearchParams(requestParams.replace(/^\?/, ''))
	return {
		query: params.get('query') ?? undefined,
		offset: params.get('offset') ?? undefined,
		index: params.get('index') ?? undefined,
		limit: params.get('limit') ?? undefined,
		new_filters: params.get('new_filters') ?? undefined,
	}
}

function readDisplayMode(): DisplayMode {
	const stored = localStorage.getItem(DISPLAY_MODE_KEY)
	return DISPLAY_MODES.find((mode) => mode === stored) ?? 'list'
}

function browseFacts(list: ContentListResponse): string {
	return [
		list.content_type,
		list.loader,
		list.game_version,
		String(list.permissions.can_write),
		installedFacts(installedProjects(list)),
	].join('|')
}

export interface BrowseManagerOptions {
	serverId: Ulid
	socket: ServerEventSource
	busyReasons: Ref<BusyReason[]> | ComputedRef<BusyReason[]>
	back: RouteLocationRaw
	client?: PanelApi
}

export interface BrowseManagerHandle {
	ready: ComputedRef<boolean>
	loadError: Ref<Error | null>
	searchError: Ref<Error | null>
	load: () => Promise<void>
	refreshSearch: () => Promise<void>
	displayMode: Ref<DisplayMode>
}

export function useBrowseManager(options: BrowseManagerOptions): BrowseManagerHandle {
	const { formatMessage } = useVIntl()
	const { addNotification } = injectNotificationManager()
	const modrinth = injectModrinthClient()
	const router = useRouter()
	const client = options.client ?? api
	const { isBusy, busyMessage } = useBusyState(options.busyReasons)

	const lifetime = new AbortController()

	const tags = ref<Tags>({ gameVersions: [], loaders: [], categories: [] })
	const list = ref<ContentListResponse | null>(null)
	const loadError = ref<Error | null>(null)
	const searchError = ref<Error | null>(null)
	const ready = computed(() => tags.value.loaders.length > 0 && list.value !== null)

	const query = useRoute().query
	const selection = ref(new Map<string, BrowseSelectedProject>())
	const installing = ref(false)
	const hideInstalled = ref(query.hi === 'true')
	const hideSelected = ref(false)
	const serverOnly = ref(query.so === 'true')
	const advancedFiltersCollapsed = ref(true)
	const filtersMenuOpen = ref(false)
	const displayMode = ref<DisplayMode>(readDisplayMode())

	const projectType = computed<string>(() => list.value?.content_type ?? 'mod')
	const canWrite = computed(() => list.value?.permissions.can_write ?? false)
	const projects = computed(() => installedProjects(list.value))
	const installedIds = computed(
		() => new Set([...projects.value.installed, ...projects.value.installing]),
	)

	const providedFilters = computed<FilterValue[]>(() => {
		const filters: FilterValue[] = []
		const gameVersion = list.value?.game_version
		if (gameVersion) filters.push({ type: 'game_version', option: gameVersion })
		filters.push(...loaderFacet(projectType.value, list.value?.loader ?? null, tags.value))

		const hidden = new Set<string>()
		if (hideInstalled.value) for (const id of installedIds.value) hidden.add(id)
		if (hideSelected.value) for (const id of selection.value.keys()) hidden.add(id)
		for (const id of hidden) {
			filters.push({ type: 'project_id', option: `project_id:${id}`, negative: true })
		}
		return filters
	})

	const showServerOnly = computed(() => projectType.value === 'mod')
	const environmentOverride = computed<EnvironmentSearchOverride | undefined>(() => {
		if (!showServerOnly.value) return undefined
		if (serverOnly.value) {
			return {
				mode: 'include',
				values: [
					'server_only',
					'dedicated_server_only',
					'server_only_client_optional',
					'client_or_server_prefers_both',
					'client_or_server',
				],
			}
		}
		return { mode: 'exclude', values: ['client_only', 'singleplayer_only'] }
	})

	let searches = 0

	async function search(requestParams: string): Promise<BrowseSearchResponse> {
		const attempt = ++searches
		try {
			const results = await modrinth.request<Labrinth.Search.v3.SearchResults>('/search', {
				api: 'labrinth',
				version: 3,
				method: 'GET',
				params: searchParams(requestParams),
				signal: lifetime.signal,
			})
			if (attempt === searches) searchError.value = null
			return {
				projectHits: results.hits,
				serverHits: [],
				total_hits: results.total_hits,
				per_page: results.hits_per_page,
			}
		} catch (cause) {
			if (attempt === searches && !lifetime.signal.aborted) {
				searchError.value = cause instanceof Error ? cause : new Error(String(cause))
			}
			throw cause
		}
	}

	const searchState = useBrowseSearch({
		projectType,
		tags,
		providedFilters,
		environmentOverride,
		active: ready,
		search,
		persistentQueryParams: [],
		getExtraQueryParams: () => ({
			so: showServerOnly.value && serverOnly.value ? 'true' : undefined,
			hi: hideInstalled.value ? 'true' : undefined,
		}),
		maxResultsOptions: computed(() => MAX_RESULTS[displayMode.value]),
		displayMode,
	})

	function adoptList(fetched: ContentListResponse): void {
		list.value = fetched
		const kept = stillSelectable(selection.value, projects.value)
		if (kept.size !== selection.value.size) selection.value = kept
	}

	async function load(): Promise<void> {
		loadError.value = null
		try {
			const [fetchedTags, fetchedList] = await Promise.all([
				readTags(modrinth, lifetime.signal),
				client.content.list(options.serverId, {}, { signal: lifetime.signal }),
			])
			tags.value = fetchedTags
			adoptList(fetchedList)
		} catch (cause) {
			if (isAbortError(cause) || lifetime.signal.aborted) return
			loadError.value = cause instanceof Error ? cause : new Error(String(cause))
		}
	}

	async function reloadList(): Promise<void> {
		if (list.value === null) return
		try {
			const fetched = await client.content.list(options.serverId, {}, { signal: lifetime.signal })
			if (list.value !== null && browseFacts(fetched) === browseFacts(list.value)) return
			adoptList(fetched)
		} catch {
		}
	}

	function toggle(result: Labrinth.Search.v3.ResultSearchProject): void {
		const selected = selection.value.has(result.project_id)
		if (!isSelectable(installState(result.project_id, projects.value, selected))) return

		const next = new Map(selection.value)
		if (!next.delete(result.project_id)) {
			next.set(result.project_id, {
				id: result.project_id,
				name: result.name,
				iconUrl: result.icon_url,
			})
		}
		selection.value = next
	}

	function clearSelection(): void {
		selection.value = new Map()
	}

	function reportSkipped(skipped: ContentSkippedEntry[]): void {
		if (skipped.length === 0) return
		addNotification({
			type: 'warning',
			title: formatMessage(messages.skippedTitle),
			text: skipped
				.map((entry) =>
					formatMessage(messages.skippedEntry, {
						name: selection.value.get(entry.project_id)?.name ?? entry.project_id,
						reason: formatMessage(SKIP_REASONS[entry.reason]),
					}),
				)
				.join('\n'),
		})
	}

	async function installSelected(): Promise<void> {
		if (installing.value || selection.value.size === 0) return
		installing.value = true
		const chosen = Array.from(selection.value.keys())

		try {
			const accepted = await client.content.install(options.serverId, {
				items: chosen.map((projectId) => ({ project_id: projectId, version_id: null })),
				resolve_dependencies: true,
			})
			reportSkipped(accepted.skipped)

			if (accepted.planned.length === 0) return

			addNotification({
				type: 'success',
				title: formatMessage(messages.installStarted, { count: accepted.planned.length }),
				text: formatMessage(messages.installStartedText),
			})
			clearSelection()
			await router.push(options.back)
		} catch (cause) {
			addNotification({
				type: 'error',
				title: formatMessage(messages.installFailed),
				text: cause instanceof Error ? cause.message : undefined,
			})
		} finally {
			installing.value = false
		}
	}

	async function discardAndBack(): Promise<void> {
		clearSelection()
		await router.push(options.back)
	}

	function installAction(result: Labrinth.Search.v3.ResultSearchProject): CardAction {
		const selected = selection.value.has(result.project_id)
		const state = installState(result.project_id, projects.value, selected)
		const running = state === 'installing' || (installing.value && selected)
		const blocked = !isSelectable(state) || installing.value || isBusy.value || !canWrite.value

		return {
			key: 'install',
			label: formatMessage(
				state === 'installed'
					? commonMessages.installedLabel
					: running
						? commonMessages.installingLabel
						: selected
							? commonMessages.selectedLabel
							: commonMessages.installButton,
			),
			icon: running ? SpinnerIcon : state === 'installed' || selected ? CheckIcon : DownloadIcon,
			iconClass: running ? 'animate-spin' : undefined,
			disabled: blocked,
			color: selected ? 'green' : 'brand',
			type: 'outlined',
			tooltip: canWrite.value
				? (busyMessage.value ?? undefined)
				: formatMessage(messages.noWritePermission),
			onClick: () => toggle(result),
		}
	}

	const installContext = computed<BrowseInstallContext>(() => ({
		name: formatMessage(commonMessages.installingContentLabel),
		loader: list.value?.loader ?? '',
		gameVersion: list.value?.game_version ?? '',
		serverId: null,
		upstream: null,
		iconSrc: null,
		backUrl: options.back,
		backLabel: formatMessage(messages.back),
		heading: '',
		queuedCount: selection.value.size,
		selectedProjects: Array.from(selection.value.values()),
		isInstallingSelected: installing.value,
		clearSelected: clearSelection,
		clearQueued: clearSelection,
		discardSelectedAndBack: discardAndBack,
		installSelected,
	}))

	watch(displayMode, (mode) => localStorage.setItem(DISPLAY_MODE_KEY, mode))

	const stopContentChanged = options.socket.on('content_changed', () => void reloadList())

	onScopeDispose(() => {
		stopContentChanged()
		lifetime.abort()
	})

	const context: BrowseManagerContext = {
		tags,
		projectType,
		...searchState,
		getProjectLink: (result) => projectUrl(projectType.value, result.slug ?? result.project_id),
		getServerProjectLink: (result) => projectUrl('server', result.slug ?? result.project_id),
		selectableProjectTypes: computed(() => []),
		showProjectTypeTabs: computed(() => false),
		variant: 'web',
		getCardActions: (result) => [installAction(result)],
		installContext,
		providedFilters,
		hideInstalled,
		showHideInstalled: computed(() => installedIds.value.size > 0),
		hideSelected,
		showHideSelected: computed(() => selection.value.size > 0),
		serverOnly,
		showServerOnly,
		hiddenFilterTypes: computed(() => (showServerOnly.value ? ['environment'] : [])),
		advancedFiltersCollapsed,
		filtersMenuOpen,
		displayMode,
		cycleDisplayMode: () => {
			const next = DISPLAY_MODES.indexOf(displayMode.value) + 1
			displayMode.value = DISPLAY_MODES[next % DISPLAY_MODES.length]
		},
		maxResultsOptions: computed(() => MAX_RESULTS[displayMode.value]),
		offline: computed(() => searchError.value !== null),
		lockedFilterMessages: {
			gameVersion: formatMessage(messages.lockedGameVersion),
			modLoader: formatMessage(messages.lockedLoader),
			providedBy: formatMessage(messages.lockedProvidedBy),
			syncButton: formatMessage(messages.lockedSync),
		},
	}

	provideBrowseManager(context)

	void load()

	return {
		ready,
		loadError,
		searchError,
		load,
		refreshSearch: searchState.refreshSearch,
		displayMode,
	}
}
