import type { Mclogs } from '@modrinth/api-client'
import { useVIntl } from '@modrinth/ui/src/composables/i18n.ts'
import { hasServerPermission } from '@modrinth/ui/src/composables/server-permissions.ts'
import { detectLogLevel } from '@modrinth/ui/src/layouts/shared/console/composables/log-level.ts'
import type { ConsoleManagerContext } from '@modrinth/ui/src/layouts/shared/console/providers/console-manager.ts'
import { provideConsoleManager } from '@modrinth/ui/src/layouts/shared/console/providers/index.ts'
import type { LogLevel, LogLine, LogSource } from '@modrinth/ui/src/layouts/shared/console/types.ts'
import { injectNotificationManager } from '@modrinth/ui/src/providers/web-notifications.ts'
import { commonMessages } from '@modrinth/ui/src/utils/common-messages.ts'
import { useStorage } from '@vueuse/core'
import type { MaybeRefOrGetter, Ref, ShallowRef } from 'vue'
import { computed, onScopeDispose, ref, shallowRef, toValue, triggerRef, watch } from 'vue'

import type {
	CrashAnalysisResponse,
	LogFile,
	PermissionMask,
	PowerState,
	ServerEventSource,
	ServerSocketStatus,
	Ulid,
} from '@/api'
import {
	api,
	CONSOLE_CLIENT_BUFFER_BYTES,
	CONSOLE_CLIENT_BUFFER_LINES,
	hasErrorCode,
	isApiRequestError,
	PANEL_LINE_TAG,
} from '@/api'

const MAX_LINES = CONSOLE_CLIENT_BUFFER_LINES
const MAX_CHARS = CONSOLE_CLIENT_BUFFER_BYTES
const KEEP_LINES = Math.floor(MAX_LINES * 0.8)
const KEEP_CHARS = Math.floor(MAX_CHARS * 0.8)

const ENTRY_START = /^\[\d{2}:\d{2}:\d{2}\]/

const LIVE_SOURCE_ID = 'live'
const LIVE_SOURCE_NAME = 'Live Log'
const LATEST_LOG = 'logs/latest.log'
const CRASH_DISMISS_MS = 30 * 60 * 1000

class ConsoleBuffer {
	lines: LogLine[] = []
	private chars = 0
	private parentLevel: LogLevel | null = null

	append(texts: string[]): boolean {
		for (const text of texts) {
			const level = detectLogLevel(text)
			if (ENTRY_START.test(text)) {
				this.parentLevel = level
				this.lines.push({ text, level })
			} else {
				this.lines.push({ text, level: level ?? this.parentLevel })
			}
			this.chars += text.length + 1
		}
		return this.trim()
	}

	reset(): void {
		this.lines = []
		this.chars = 0
		this.parentLevel = null
	}

	private trim(): boolean {
		if (this.lines.length <= MAX_LINES && this.chars <= MAX_CHARS) return false
		let dropped = 0
		while (
			dropped < this.lines.length &&
			(this.lines.length - dropped > KEEP_LINES || this.chars > KEEP_CHARS)
		) {
			this.chars -= this.lines[dropped].text.length + 1
			dropped += 1
		}
		this.lines = this.lines.slice(dropped)
		return true
	}
}

function timestamp(): string {
	const now = new Date()
	const parts = [now.getHours(), now.getMinutes(), now.getSeconds()]
	return `[${parts.map((part) => String(part).padStart(2, '0')).join(':')}]`
}

function gapNotice(count: number): string {
	const plural = count === 1 ? '' : 's'
	return `${timestamp()} [${PANEL_LINE_TAG}/WARN]: ${count} line${plural} lost before this point`
}

function truncationNotice(): string {
	return `${timestamp()} [${PANEL_LINE_TAG}/WARN]: older lines omitted, this file is too long for the console`
}

function splitContent(content: string): string[] {
	const lines = content.split(/\r?\n/)
	if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()
	return lines
}

function messageOf(error: unknown): string {
	if (isApiRequestError(error)) return error.message
	return error instanceof Error ? error.message : 'Unknown error.'
}

function toInsights(response: CrashAnalysisResponse): Mclogs.Insights.v1.InsightsResponse {
	return { ...response, name: response.name ?? '', version: response.version ?? '' }
}

export interface ConsoleManagerOptions {
	serverId: Ulid
	socket: ServerEventSource
	powerState: MaybeRefOrGetter<PowerState>
	permissions: MaybeRefOrGetter<PermissionMask>
	externalServicesEnabled?: MaybeRefOrGetter<boolean>
	client?: typeof api.console
}

export interface ConsoleManager {
	context: ConsoleManagerContext
	logLines: ShallowRef<LogLine[]>
	socketStatus: Ref<ServerSocketStatus>
	logFiles: Ref<LogFile[]>
	logFilesTruncated: Ref<boolean>
	historyIncomplete: Ref<boolean>
	crashAnalysis: Ref<Mclogs.Insights.v1.InsightsResponse | null>
	refreshLogFiles: () => Promise<void>
	analyseCrash: () => Promise<void>
}

export function useConsoleManager(options: ConsoleManagerOptions): ConsoleManager {
	const { serverId, socket } = options
	const client = options.client ?? api.console
	const { addNotification } = injectNotificationManager()
	const { formatMessage } = useVIntl()

	const aborter = new AbortController()
	const reading = { signal: aborter.signal }
	const gone = () => aborter.signal.aborted

	const live = new ConsoleBuffer()
	const opened = new ConsoleBuffer()
	const logLines = shallowRef<LogLine[]>(live.lines)

	const logFiles = ref<LogFile[]>([])
	const logFilesTruncated = ref(false)
	const activeLogSourceIndex = ref(0)
	const activeSourceId = ref(LIVE_SOURCE_ID)
	const isLive = computed(() => activeSourceId.value === LIVE_SOURCE_ID)

	const socketStatus = ref<ServerSocketStatus>(socket.status)
	const historyStreaming = ref(socket.status.phase !== 'open')
	const historyIncomplete = ref(false)
	const fileLoading = ref(false)

	const crashAnalysis = ref<Mclogs.Insights.v1.InsightsResponse | null>(null)
	const dismissedUntil = useStorage(`craftpanel-crash-dismissed-${serverId}`, 0)

	const powerState = computed(() => toValue(options.powerState))
	const isRunning = computed(() => powerState.value === 'running')
	const processAlive = computed(
		() => powerState.value === 'starting' || powerState.value === 'stopping' || isRunning.value,
	)

	const permissions = computed(() => toValue(options.permissions))
	const canExecuteCommands = computed(() => hasServerPermission(permissions.value, 'EXEC_COMMANDS'))
	const canWriteFiles = computed(() => hasServerPermission(permissions.value, 'FILES_WRITE'))
	const noPermission = computed(() => formatMessage(commonMessages.noPermissionAction))

	const connected = computed(() => socketStatus.value.phase === 'open')
	const unreachable = computed(
		() =>
			socketStatus.value.givenUp ||
			socketStatus.value.authIncorrect ||
			(socketStatus.value.phase === 'closed' && socketStatus.value.closeCode !== null),
	)
	const externalServices = computed(() => toValue(options.externalServicesEnabled ?? true))

	function showBuffer(buffer: ConsoleBuffer): void {
		logLines.value = buffer.lines
		triggerRef(logLines)
	}

	function publish(buffer: ConsoleBuffer, replaced: boolean): void {
		if (replaced) logLines.value = buffer.lines
		else triggerRef(logLines)
	}

	const visibleFiles = computed(() =>
		logFiles.value.filter((entry) => !(processAlive.value && entry.file === LATEST_LOG)),
	)

	const logSources = computed<LogSource[]>(() => [
		{ id: LIVE_SOURCE_ID, name: LIVE_SOURCE_NAME, live: true },
		...visibleFiles.value.map((entry) => ({ id: entry.file, name: entry.name, live: false })),
	])

	const activeFile = computed(() =>
		visibleFiles.value.find((entry) => entry.file === activeSourceId.value),
	)

	const loading = computed(
		() =>
			fileLoading.value ||
			(isLive.value && !unreachable.value && (historyStreaming.value || !connected.value)),
	)

	const listeners = [
		socket.on('status', (status) => {
			socketStatus.value = status
			if (status.phase === 'open') historyStreaming.value = true
		}),
		socket.on('console_history_start', (message) => {
			historyStreaming.value = true
			historyIncomplete.value = message.dropped_lines > 0
			live.reset()
			if (isLive.value) showBuffer(live)
		}),
		socket.on('console', (block) => {
			if (block.missing > 0) {
				historyIncomplete.value = true
				live.append([gapNotice(block.missing)])
			}
			const replaced = live.append(block.lines)
			if (block.history || !isLive.value) return
			publish(live, replaced)
		}),
		socket.on('console_history_end', () => {
			historyStreaming.value = false
			if (isLive.value) showBuffer(live)
		}),
		socket.on('console_cleared', () => {
			live.reset()
			if (isLive.value) showBuffer(live)
		}),
	]

	async function refreshLogFiles(): Promise<void> {
		try {
			const page = await client.listLogs(serverId, {}, reading)
			logFiles.value = page.files
			logFilesTruncated.value = page.truncated
		} catch (error) {
			if (gone()) return
			addNotification({
				title: 'Could not load log files',
				text: messageOf(error),
				type: 'error',
			})
		}
	}

	function showLive(): void {
		activeSourceId.value = LIVE_SOURCE_ID
		opened.reset()
		fileLoading.value = false
		showBuffer(live)
	}

	async function selectSource(index: number): Promise<void> {
		const source = logSources.value[index]
		if (!source || source.live) {
			showLive()
			return
		}
		if (source.id === activeSourceId.value) return

		activeSourceId.value = source.id
		fileLoading.value = true
		opened.reset()
		showBuffer(opened)

		try {
			const response = await client.readLog(serverId, source.id, reading)
			if (activeSourceId.value !== source.id) return
			const lines = splitContent(response.content)
			if (response.truncated) lines.unshift(truncationNotice())
			const replaced = opened.append(lines)
			publish(opened, replaced)
		} catch (error) {
			if (gone() || activeSourceId.value !== source.id) return
			addNotification({ title: 'Could not open log file', text: messageOf(error), type: 'error' })
			activeLogSourceIndex.value = 0
			showLive()
		} finally {
			if (activeSourceId.value === source.id) fileLoading.value = false
		}
	}

	function sendCommand(command: string): void {
		void client.sendCommand(serverId, { command }).catch((error: unknown) => {
			addNotification({ title: 'Command failed', text: messageOf(error), type: 'error' })
		})
	}

	function clearConsole(): void {
		live.reset()
		if (isLive.value) showBuffer(live)
		void client.clear(serverId).catch((error: unknown) => {
			addNotification({
				title: 'Could not clear the console',
				text: messageOf(error),
				type: 'error',
			})
		})
	}

	async function deleteActiveLog(): Promise<void> {
		const target = activeFile.value
		if (!target) return
		try {
			await client.deleteLog(serverId, target.file)
		} catch (error) {
			throw messageOf(error)
		}
		activeLogSourceIndex.value = 0
		showLive()
		await refreshLogFiles()
	}

	async function requestAnalysis(): Promise<CrashAnalysisResponse> {
		try {
			return await client.crashAnalysis(serverId, { source: 'latest_log' }, reading)
		} catch (error) {
			if (!hasErrorCode(error, 'log_file_missing')) throw error
			return await client.crashAnalysis(serverId, { source: 'buffer' }, reading)
		}
	}

	async function analyseCrash(): Promise<void> {
		if (Date.now() < dismissedUntil.value) return
		const asked = powerState.value
		try {
			const response = await requestAnalysis()
			if (powerState.value !== asked) return
			crashAnalysis.value = response.analysis.problems.length > 0 ? toInsights(response) : null
		} catch {
			if (gone() || powerState.value !== asked) return
			crashAnalysis.value = null
		}
	}

	function dismissCrash(): void {
		dismissedUntil.value = Date.now() + CRASH_DISMISS_MS
		crashAnalysis.value = null
	}

	watch(activeLogSourceIndex, (index) => {
		void selectSource(index)
	})

	watch(logSources, (sources) => {
		if (isLive.value) return
		const index = sources.findIndex((source) => source.id === activeSourceId.value)
		if (index === activeLogSourceIndex.value) return
		if (index === -1) showLive()
		activeLogSourceIndex.value = Math.max(index, 0)
	})

	watch(processAlive, () => {
		void refreshLogFiles()
	})

	watch(
		powerState,
		(state) => {
			if (state === 'crashed') void analyseCrash()
			else crashAnalysis.value = null
		},
		{ immediate: true },
	)

	void refreshLogFiles()

	onScopeDispose(() => {
		for (const off of listeners) off()
		aborter.abort()
	})

	const context: ConsoleManagerContext = {
		logLines,
		logSources,
		activeLogSourceIndex,
		sendCommand,
		showCommandInput: isLive,
		disableCommandInput: computed(() => !canExecuteCommands.value || !isRunning.value),
		disableCommandInputTooltip: computed(() =>
			canExecuteCommands.value ? undefined : noPermission.value,
		),
		loading,
		onClear: clearConsole,
		clearDisabled: computed(() => !canExecuteCommands.value),
		clearDisabledTooltip: computed(() =>
			canExecuteCommands.value ? undefined : noPermission.value,
		),
		onDelete: deleteActiveLog,
		deleteDisabled: computed(
			() => !canWriteFiles.value || (activeSourceId.value === LATEST_LOG && processAlive.value),
		),
		get deleteDisabledTooltip(): string | undefined {
			if (!canWriteFiles.value) return noPermission.value
			if (activeSourceId.value === LATEST_LOG && processAlive.value) {
				return 'The current log cannot be deleted while the server is running.'
			}
			return undefined
		},
		shareDisabled: computed(() => !connected.value || !externalServices.value),
		emptyStateType: 'server',
		crashAnalysis,
		onDismissCrash: dismissCrash,
	}

	provideConsoleManager(context)

	return {
		context,
		logLines,
		socketStatus,
		logFiles,
		logFilesTruncated,
		historyIncomplete,
		crashAnalysis,
		refreshLogFiles,
		analyseCrash,
	}
}
