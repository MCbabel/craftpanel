import { API_BASE } from './client'
import type {
	Ulid,
	WsBackupListChangedMessage,
	WsConsoleClearedMessage,
	WsConsoleHistoryEndMessage,
	WsConsoleHistoryStartMessage,
	WsConsoleMessage,
	WsContentChangedMessage,
	WsMessage,
	WsNetworkChangedMessage,
	WsOperationsMessage,
	WsServerMessage,
	WsStartupChangedMessage,
	WsStateMessage,
	WsStatsMessage,
} from './types'

export function serverSocketUrl(serverId: Ulid, origin: string = location.origin): string {
	const url = new URL(`${API_BASE}/servers/${encodeURIComponent(serverId)}/ws`, origin)
	url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
	return url.toString()
}

export interface ConsoleLines {
	seq: number
	lines: string[]
	missing: number
	history: boolean
}

export class ConsoleSequence {
	private expected: number | null = null

	reset(): void {
		this.expected = null
	}

	accept(message: WsConsoleMessage): Omit<ConsoleLines, 'history'> | null {
		const { seq, lines } = message
		if (lines.length === 0) return null

		if (this.expected === null) {
			this.expected = seq + lines.length
			return { seq, lines, missing: 0 }
		}
		if (seq + lines.length <= this.expected) return null

		if (seq >= this.expected) {
			const missing = seq - this.expected
			this.expected = seq + lines.length
			return { seq, lines, missing }
		}

		const seen = this.expected - seq
		const first = this.expected
		this.expected = seq + lines.length
		return { seq: first, lines: lines.slice(seen), missing: 0 }
	}
}

export interface ServerSocketStatus {
	phase: 'connecting' | 'open' | 'waiting' | 'closed'
	authIncorrect: boolean
	closeCode: number | null
	attempts: number
	givenUp: boolean
}

export interface ServerSocketEventMap {
	server: WsServerMessage
	state: WsStateMessage
	stats: WsStatsMessage
	operations: WsOperationsMessage
	console_history_start: WsConsoleHistoryStartMessage
	console: ConsoleLines
	console_history_end: WsConsoleHistoryEndMessage
	console_cleared: WsConsoleClearedMessage
	content_changed: WsContentChangedMessage
	backup_list_changed: WsBackupListChangedMessage
	startup_changed: WsStartupChangedMessage
	network_changed: WsNetworkChangedMessage
	status: ServerSocketStatus
}

export type ServerSocketEvent = keyof ServerSocketEventMap

export interface ServerEventSource {
	readonly status: ServerSocketStatus
	on<K extends ServerSocketEvent>(
		type: K,
		listener: (payload: ServerSocketEventMap[K]) => void,
	): () => void
}

export interface TransportHandlers {
	onOpen: () => void
	onMessage: (data: string) => void
	onClose: (code: number) => void
}

export interface Transport {
	close: (code?: number) => void
}

export type TransportFactory = (url: string, handlers: TransportHandlers) => Transport

const browserTransport: TransportFactory = (url, handlers) => {
	const socket = new WebSocket(url)
	socket.addEventListener('open', () => handlers.onOpen())
	socket.addEventListener('message', (event) => {
		if (typeof event.data === 'string') handlers.onMessage(event.data)
	})
	socket.addEventListener('close', (event) => handlers.onClose(event.code))
	return { close: (code) => socket.close(code) }
}

export interface BackoffOptions {
	initialDelayMs: number
	maxDelayMs: number
	factor: number
	jitter: number
	maxAttempts: number
}

export const DEFAULT_BACKOFF: BackoffOptions = {
	initialDelayMs: 1_000,
	maxDelayMs: 30_000,
	factor: 2,
	jitter: 0.2,
	maxAttempts: 8,
}

export interface ServerSocketOptions {
	url?: string
	transport?: TransportFactory
	backoff?: Partial<BackoffOptions>
	random?: () => number
}

const FINAL_CLOSE_CODES = new Set([1000, 4401, 4403, 4404, 4429])
const AUTH_CLOSE_CODES = new Set([4401, 4403])

const MESSAGE_TYPES = new Set<string>([
	'server',
	'state',
	'stats',
	'operations',
	'console_history_start',
	'console',
	'console_history_end',
	'console_cleared',
	'content_changed',
	'backup_list_changed',
	'startup_changed',
	'network_changed',
])

function parseMessage(data: string): WsMessage | null {
	let parsed: unknown
	try {
		parsed = JSON.parse(data)
	} catch {
		return null
	}
	if (typeof parsed !== 'object' || parsed === null || !('type' in parsed)) return null
	if (typeof parsed.type !== 'string' || !MESSAGE_TYPES.has(parsed.type)) return null
	return parsed as WsMessage
}

type ListenerGroups = {
	[K in ServerSocketEvent]: Set<(payload: ServerSocketEventMap[K]) => void>
}

function emptyListenerGroups(): ListenerGroups {
	return {
		server: new Set(),
		state: new Set(),
		stats: new Set(),
		operations: new Set(),
		console_history_start: new Set(),
		console: new Set(),
		console_history_end: new Set(),
		console_cleared: new Set(),
		content_changed: new Set(),
		backup_list_changed: new Set(),
		startup_changed: new Set(),
		network_changed: new Set(),
		status: new Set(),
	}
}

export class ServerSocket implements ServerEventSource {
	private readonly url: string
	private readonly transportFactory: TransportFactory
	private readonly backoff: BackoffOptions
	private readonly random: () => number
	private readonly listeners: ListenerGroups = emptyListenerGroups()
	private readonly sequence = new ConsoleSequence()

	private transport: Transport | null = null
	private timer: ReturnType<typeof setTimeout> | null = null
	private stopped = false
	private inHistory = false
	private state: ServerSocketStatus = {
		phase: 'closed',
		authIncorrect: false,
		closeCode: null,
		attempts: 0,
		givenUp: false,
	}

	constructor(serverId: Ulid, options: ServerSocketOptions = {}) {
		this.url = options.url ?? serverSocketUrl(serverId)
		this.transportFactory = options.transport ?? browserTransport
		this.backoff = { ...DEFAULT_BACKOFF, ...options.backoff }
		this.random = options.random ?? Math.random
	}

	get status(): ServerSocketStatus {
		return this.state
	}

	on<K extends ServerSocketEvent>(
		type: K,
		listener: (payload: ServerSocketEventMap[K]) => void,
	): () => void {
		const group = this.listeners[type]
		group.add(listener)
		return () => {
			group.delete(listener)
		}
	}

	connect(): void {
		if (this.transport || this.timer) return
		this.stopped = false
		this.open({ attempts: 0, givenUp: false, authIncorrect: false, closeCode: null })
	}

	close(): void {
		this.stopped = true
		this.clearTimer()
		this.transport?.close(1000)
		this.transport = null
		this.setStatus({ phase: 'closed' })
	}

	private open(patch: Partial<ServerSocketStatus> = {}): void {
		this.setStatus({ phase: 'connecting', ...patch })
		this.transport = this.transportFactory(this.url, {
			onOpen: () => this.setStatus({ phase: 'open', attempts: 0 }),
			onMessage: (data) => this.dispatch(data),
			onClose: (code) => this.handleClose(code),
		})
	}

	private handleClose(code: number): void {
		this.transport = null
		this.inHistory = false
		const authIncorrect = AUTH_CLOSE_CODES.has(code)

		if (this.stopped || FINAL_CLOSE_CODES.has(code)) {
			this.setStatus({ phase: 'closed', closeCode: code, authIncorrect })
			return
		}

		const attempts = this.state.attempts + 1
		if (attempts > this.backoff.maxAttempts) {
			this.setStatus({ phase: 'closed', closeCode: code, attempts, givenUp: true })
			return
		}

		this.setStatus({ phase: 'waiting', closeCode: code, attempts })
		this.timer = setTimeout(() => {
			this.timer = null
			if (!this.stopped) this.open()
		}, this.delayFor(attempts))
	}

	private delayFor(attempt: number): number {
		const { initialDelayMs, maxDelayMs, factor, jitter } = this.backoff
		const base = Math.min(maxDelayMs, initialDelayMs * factor ** (attempt - 1))
		return Math.round(base * (1 + (this.random() * 2 - 1) * jitter))
	}

	private clearTimer(): void {
		if (this.timer === null) return
		clearTimeout(this.timer)
		this.timer = null
	}

	private dispatch(data: string): void {
		const message = parseMessage(data)
		if (!message) return

		switch (message.type) {
			case 'console_history_start':
				this.sequence.reset()
				this.inHistory = true
				this.emit('console_history_start', message)
				return
			case 'console_history_end':
				this.inHistory = false
				this.emit('console_history_end', message)
				return
			case 'console': {
				const slice = this.sequence.accept(message)
				if (slice) this.emit('console', { ...slice, history: this.inHistory })
				return
			}
			case 'server':
				this.emit('server', message)
				return
			case 'state':
				this.emit('state', message)
				return
			case 'stats':
				this.emit('stats', message)
				return
			case 'operations':
				this.emit('operations', message)
				return
			case 'console_cleared':
				this.emit('console_cleared', message)
				return
			case 'content_changed':
				this.emit('content_changed', message)
				return
			case 'backup_list_changed':
				this.emit('backup_list_changed', message)
				return
			case 'startup_changed':
				this.emit('startup_changed', message)
				return
			case 'network_changed':
				this.emit('network_changed', message)
		}
	}

	private setStatus(patch: Partial<ServerSocketStatus>): void {
		this.state = { ...this.state, ...patch }
		this.emit('status', this.state)
	}

	private emit<K extends ServerSocketEvent>(type: K, payload: ServerSocketEventMap[K]): void {
		for (const listener of [...this.listeners[type]]) listener(payload)
	}
}

export function openServerSocket(serverId: Ulid, options: ServerSocketOptions = {}): ServerSocket {
	const socket = new ServerSocket(serverId, options)
	socket.connect()
	return socket
}
