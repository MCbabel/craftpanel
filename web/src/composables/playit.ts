import { onScopeDispose, ref, type Ref } from 'vue'

import { isApiRequestError, type Ulid } from '@/api'
import { playit, playitAbsent, type ServerTunnel, tunnelPollMs } from '@/api/playit'

export interface ServerTunnelHandle {
	tunnel: Ref<ServerTunnel | null>
	loading: Ref<boolean>
	error: Ref<string | null>
	available: Ref<boolean>
	refresh: () => Promise<void>
	request: () => Promise<void>
	remove: () => Promise<void>
}

interface Entry {
	tunnel: Ref<ServerTunnel | null>
	loading: Ref<boolean>
	error: Ref<string | null>
	watchers: number
	timer: ReturnType<typeof setTimeout> | null
	inflight: Promise<void> | null
}

const available = ref(true)
const entries = new Map<Ulid, Entry>()

function entryFor(serverId: Ulid): Entry {
	let entry = entries.get(serverId)
	if (entry === undefined) {
		entry = {
			tunnel: ref(null),
			loading: ref(false),
			error: ref(null),
			watchers: 0,
			timer: null,
			inflight: null,
		}
		entries.set(serverId, entry)
	}
	return entry
}

function read(serverId: Ulid, entry: Entry): Promise<void> {
	entry.inflight ??= (async () => {
		entry.loading.value = true
		try {
			entry.tunnel.value = await playit.tunnel(serverId)
			entry.error.value = null
		} catch (cause) {
			if (playitAbsent(cause)) {
				available.value = false
				entry.error.value = null
			} else {
				entry.error.value = isApiRequestError(cause) ? cause.message : String(cause)
			}
		} finally {
			entry.loading.value = false
			entry.inflight = null
		}
	})()
	return entry.inflight
}

function schedule(serverId: Ulid, entry: Entry): void {
	if (entry.timer !== null) clearTimeout(entry.timer)
	entry.timer = null
	if (entry.watchers === 0) return

	const wait = tunnelPollMs(entry.tunnel.value, available.value)
	if (wait === null) return
	entry.timer = setTimeout(() => {
		void read(serverId, entry).then(() => schedule(serverId, entry))
	}, wait)
}

export function useServerTunnel(serverId: Ulid): ServerTunnelHandle {
	const entry = entryFor(serverId)
	entry.watchers += 1

	async function refresh(): Promise<void> {
		if (!available.value) return
		await read(serverId, entry)
		schedule(serverId, entry)
	}

	void refresh()

	onScopeDispose(() => {
		entry.watchers -= 1
		if (entry.watchers > 0) return
		if (entry.timer !== null) clearTimeout(entry.timer)
		entry.timer = null
	})

	return {
		tunnel: entry.tunnel,
		loading: entry.loading,
		error: entry.error,
		available,
		refresh,
		async request(): Promise<void> {
			entry.tunnel.value = await playit.createTunnel(serverId)
			entry.error.value = null
			schedule(serverId, entry)
		},
		async remove(): Promise<void> {
			await playit.deleteTunnel(serverId)
			await refresh()
		},
	}
}
