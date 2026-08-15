import type { LimitDimension, Me } from '@/api'

const MIB = 1024 * 1024

export interface Gauge {
	id: LimitDimension
	used: number
	limit: number | null
	percent: number | null
	over: boolean
}

export interface AccountGauges {
	unlimited: boolean
	diskAtLeast: boolean
	memory: Gauge
	disk: Gauge
	cpu: Gauge
	pids: Gauge
}

export function gaugesFor(me: Me): AccountGauges {
	const usage = me.usage
	return {
		unlimited: me.limits === null,
		diskAtLeast: usage.disk.complete === false,
		memory: gauge('memory', usage.memory.allocated_mib, usage.memory.limit_mib, usage),
		disk: gauge('disk', usage.disk.used_bytes / MIB, usage.disk.limit_mib, usage),
		cpu: gauge('cpu', usage.cpu.used_cores, usage.cpu.limit_cores, usage),
		pids: gauge('pids', usage.pids.used, usage.pids.limit, usage),
	}
}

function gauge(
	id: LimitDimension,
	used: number,
	limit: number | null,
	usage: Me['usage'],
): Gauge {
	return {
		id,
		used,
		limit,
		percent: share(used, limit),
		over: usage.over_limit_dimensions.includes(id),
	}
}

function share(used: number, limit: number | null): number | null {
	if (limit === null) return null
	if (limit <= 0) return used > 0 ? 100 : 0
	return Math.min(100, Math.max(0, Math.round((used / limit) * 100)))
}
