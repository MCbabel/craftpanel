import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { ConsoleLines, Transport, TransportFactory, TransportHandlers } from './socket'
import { ConsoleSequence, ServerSocket } from './socket'
import type { WsConsoleMessage } from './types'

function consoleMessage(raw: string): WsConsoleMessage {
	return JSON.parse(raw) as WsConsoleMessage
}

describe('Deduplication by sequence number', () => {
	it('takes the first block as it comes', () => {
		const message = consoleMessage(
			'{ "type": "console", "seq": 8421, "lines": ["[15:04:22] [Server thread/INFO]: Done (6.213s)!"] }',
		)
		const sequence = new ConsoleSequence()

		expect(sequence.accept(message)).toEqual({ seq: 8421, lines: message.lines, missing: 0 })
	})

	it('throws away a block that is repeated in full', () => {
		const sequence = new ConsoleSequence()
		const message = consoleMessage('{"type":"console","seq":0,"lines":["a","b","c"]}')

		expect(sequence.accept(message)).not.toBeNull()
		expect(sequence.accept(message)).toBeNull()
	})

	it('cuts away the overlap between the history and the live output', () => {
		const sequence = new ConsoleSequence()
		sequence.accept(consoleMessage('{"type":"console","seq":0,"lines":["a","b","c"]}'))

		const overlap = sequence.accept(
			consoleMessage('{"type":"console","seq":1,"lines":["b","c","d","e"]}'),
		)

		expect(overlap).toEqual({ seq: 3, lines: ['d', 'e'], missing: 0 })
	})

	it('reports lost lines as a jump instead of keeping quiet about them', () => {
		const sequence = new ConsoleSequence()
		sequence.accept(consoleMessage('{"type":"console","seq":0,"lines":["a","b"]}'))

		const jumped = sequence.accept(consoleMessage('{"type":"console","seq":10,"lines":["k"]}'))

		expect(jumped).toEqual({ seq: 10, lines: ['k'], missing: 8 })
	})

	it('takes the same history again after a reset', () => {
		const sequence = new ConsoleSequence()
		sequence.accept(consoleMessage('{"type":"console","seq":8000,"lines":["a"]}'))
		sequence.reset()

		expect(
			sequence.accept(consoleMessage('{"type":"console","seq":0,"lines":["a"]}')),
		).not.toBeNull()
	})
})

class FakeTransport implements Transport {
	closedWith: number | null = null
	constructor(readonly handlers: TransportHandlers) {}
	close(code?: number): void {
		this.closedWith = code ?? 1000
	}
}

function fakeTransport(): { factory: TransportFactory; opened: FakeTransport[] } {
	const opened: FakeTransport[] = []
	const factory: TransportFactory = (_url, handlers) => {
		const transport = new FakeTransport(handlers)
		opened.push(transport)
		return transport
	}
	return { factory, opened }
}

const socketOptions = (factory: TransportFactory) => ({
	url: 'ws://panel.test/api/v1/servers/01J/ws',
	transport: factory,
	random: () => 0.5,
	backoff: { initialDelayMs: 1_000, maxDelayMs: 8_000, factor: 2, jitter: 0, maxAttempts: 4 },
})

beforeEach(() => {
	vi.useFakeTimers()
})

afterEach(() => {
	vi.restoreAllMocks()
	vi.useRealTimers()
})

describe('Reconnecting', () => {
	it('waits longer and longer, caps, and then gives up', () => {
		const { factory, opened } = fakeTransport()
		const scheduled = vi.spyOn(globalThis, 'setTimeout')
		const socket = new ServerSocket('01J', socketOptions(factory))
		socket.connect()

		for (let attempt = 0; attempt < 5; attempt += 1) {
			opened[opened.length - 1].handlers.onClose(1006)
			vi.runOnlyPendingTimers()
		}

		expect(scheduled.mock.calls.map((call) => call[1])).toEqual([1_000, 2_000, 4_000, 8_000])
		expect(opened).toHaveLength(5)
		expect(socket.status.phase).toBe('closed')
		expect(socket.status.givenUp).toBe(true)
	})

	it('connects again after 1012, not after 4401', () => {
		const restart = fakeTransport()
		const restarting = new ServerSocket('01J', socketOptions(restart.factory))
		restarting.connect()
		restart.opened[0].handlers.onClose(1012)
		vi.advanceTimersByTime(1_000)
		expect(restart.opened).toHaveLength(2)

		const denied = fakeTransport()
		const rejected = new ServerSocket('01J', socketOptions(denied.factory))
		rejected.connect()
		denied.opened[0].handlers.onClose(4401)
		vi.advanceTimersByTime(60_000)

		expect(denied.opened).toHaveLength(1)
		expect(rejected.status.authIncorrect).toBe(true)
		expect(rejected.status.closeCode).toBe(4401)
	})

	it('resets the counter as soon as a connection stood', () => {
		const { factory, opened } = fakeTransport()
		const socket = new ServerSocket('01J', socketOptions(factory))
		socket.connect()

		opened[0].handlers.onClose(1006)
		vi.advanceTimersByTime(1_000)
		opened[1].handlers.onOpen()
		expect(socket.status.attempts).toBe(0)

		opened[1].handlers.onClose(1006)
		expect(socket.status.attempts).toBe(1)
	})
})

describe('Event source', () => {
	it('brackets the history and deduplicates across the blocks', () => {
		const { factory, opened } = fakeTransport()
		const socket = new ServerSocket('01J', socketOptions(factory))
		socket.connect()
		const handlers = opened[0].handlers
		handlers.onOpen()

		const batches: ConsoleLines[] = []
		socket.on('console', (batch) => batches.push(batch))
		const historyEnded = vi.fn()
		socket.on('console_history_end', historyEnded)

		handlers.onMessage('{"type":"console_history_start","total_lines":8421,"dropped_lines":0}')
		handlers.onMessage('{"type":"console","seq":0,"lines":["one","two"]}')
		handlers.onMessage('{"type":"console_history_end"}')
		handlers.onMessage('{"type":"console","seq":0,"lines":["one","two","three"]}')

		expect(batches).toEqual([
			{ seq: 0, lines: ['one', 'two'], missing: 0, history: true },
			{ seq: 2, lines: ['three'], missing: 0, history: false },
		])
		expect(historyEnded).toHaveBeenCalledOnce()
	})

	it('takes the whole buffer again after a break', () => {
		const { factory, opened } = fakeTransport()
		const socket = new ServerSocket('01J', socketOptions(factory))
		socket.connect()
		const batches: ConsoleLines[] = []
		socket.on('console', (batch) => batches.push(batch))

		opened[0].handlers.onOpen()
		opened[0].handlers.onMessage(
			'{"type":"console_history_start","total_lines":2,"dropped_lines":0}',
		)
		opened[0].handlers.onMessage('{"type":"console","seq":40,"lines":["a","b"]}')
		opened[0].handlers.onClose(1006)
		vi.advanceTimersByTime(1_000)
		opened[1].handlers.onOpen()
		opened[1].handlers.onMessage(
			'{"type":"console_history_start","total_lines":2,"dropped_lines":0}',
		)
		opened[1].handlers.onMessage('{"type":"console","seq":40,"lines":["a","b"]}')

		expect(batches).toHaveLength(2)
		expect(batches[1]).toEqual({ seq: 40, lines: ['a', 'b'], missing: 0, history: true })
	})

	it('ignores what the contract does not know', () => {
		const { factory, opened } = fakeTransport()
		const socket = new ServerSocket('01J', socketOptions(factory))
		socket.connect()
		const seen = vi.fn()
		socket.on('state', seen)

		opened[0].handlers.onMessage('no json')
		opened[0].handlers.onMessage('{"type":"log4j","lines":[]}')
		opened[0].handlers.onMessage(
			'{"type":"state","power_state":"running","target":null,"uptime_seconds":3,"exit_code":null,"oom_killed":false}',
		)

		expect(seen).toHaveBeenCalledOnce()
	})

	it('reports the signed-out state over the status channel', () => {
		const { factory, opened } = fakeTransport()
		const socket = new ServerSocket('01J', socketOptions(factory))
		const phases: string[] = []
		socket.on('status', (status) => phases.push(status.phase))
		socket.connect()

		opened[0].handlers.onOpen()
		opened[0].handlers.onClose(4404)

		expect(phases).toEqual(['connecting', 'open', 'closed'])
		expect(socket.status.closeCode).toBe(4404)
		expect(socket.status.authIncorrect).toBe(false)
	})
})
