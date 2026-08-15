import { describe, expect, it } from 'vitest'

import type { LimitDimension, Me, UserLimits } from '@/api'

import { gaugesFor } from './limits'

const MIB = 1024 * 1024

function me(options: {
	limits: UserLimits | null
	allocatedMib: number
	memoryLimitMib: number | null
	diskUsedBytes: number
	diskLimitMib: number | null
	diskComplete?: boolean
	usedCores?: number
	coreLimit?: number | null
	pidsUsed?: number
	pidsLimit?: number | null
	over?: LimitDimension[]
}): Me {
	return {
		id: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
		username: 'max',
		avatar_url: null,
		panel_role: options.limits === null ? 'admin' : 'user',
		email: null,
		origin: 'admin',
		created_at: '2026-08-01T00:00:00Z',
		last_login_at: null,
		must_change_password: false,
		system_user: { state: 'ready', name: 'craft-max', uid: 6100, error_message: null },
		limits: options.limits,
		usage: {
			memory: {
				limit_mib: options.memoryLimitMib,
				allocated_mib: options.allocatedMib,
				used_bytes: 0,
			},
			cpu: { limit_cores: options.coreLimit ?? null, used_cores: options.usedCores ?? 0 },
			pids: { limit: options.pidsLimit ?? null, used: options.pidsUsed ?? 0 },
			disk: {
				limit_mib: options.diskLimitMib,
				used_bytes: options.diskUsedBytes,
				servers_bytes: options.diskUsedBytes,
				backups_bytes: 0,
				complete: options.diskComplete ?? true,
			},
			servers: { total: 1, running: 0 },
			over_limit: (options.over ?? []).length > 0,
			over_limit_dimensions: options.over ?? [],
			measured_at: '2026-08-13T00:00:00Z',
		},
		capabilities: {
			can_create_servers: true,
			can_start_servers: true,
			can_manage_panel_users: false,
			blocked_reason: null,
		},
		session: { id: '01ARZ3NDEKTSV4RRFFQ69G5FAV', expires_at: '2026-09-01T00:00:00Z' },
	}
}

const ordinary: UserLimits = {
	memory_mib: 4096,
	cpu_mode: 'cap',
	cpu_cores: 2,
	pids_max: 512,
	disk_mib: 51200,
}

describe('gaugesFor', () => {
	it('works out every row in percent for a limited account', () => {
		const gauges = gaugesFor(
			me({
				limits: ordinary,
				allocatedMib: 1024,
				memoryLimitMib: 4096,
				diskUsedBytes: 25600 * MIB,
				diskLimitMib: 51200,
				usedCores: 0.5,
				coreLimit: 2,
				pidsUsed: 128,
				pidsLimit: 512,
			}),
		)

		expect(gauges.unlimited).toBe(false)
		expect(gauges.memory).toMatchObject({ used: 1024, limit: 4096, percent: 25, over: false })
		expect(gauges.disk).toMatchObject({ used: 25600, limit: 51200, percent: 50 })
		expect(gauges.cpu.percent).toBe(25)
		expect(gauges.pids.percent).toBe(25)
	})

	it('names no limit and no share for an unlimited account', () => {
		const gauges = gaugesFor(
			me({
				limits: null,
				allocatedMib: 16384,
				memoryLimitMib: null,
				diskUsedBytes: 900 * 1024 * MIB,
				diskLimitMib: null,
			}),
		)

		expect(gauges.unlimited).toBe(true)
		for (const gauge of [gauges.memory, gauges.disk, gauges.cpu, gauges.pids]) {
			expect(gauge.limit).toBeNull()
			expect(gauge.percent).toBeNull()
			expect(gauge.over).toBe(false)
		}
		expect(gauges.memory.used).toBe(16384)
	})

	it('says "at least" when a directory was closed to the panel while counting', () => {
		const counted = gaugesFor(
			me({ limits: ordinary, allocatedMib: 0, memoryLimitMib: 4096, diskUsedBytes: 0, diskLimitMib: 51200 }),
		)
		expect(counted.diskAtLeast).toBe(false)

		const partial = gaugesFor(
			me({
				limits: ordinary,
				allocatedMib: 0,
				memoryLimitMib: 4096,
				diskUsedBytes: 1024 * MIB,
				diskLimitMib: 51200,
				diskComplete: false,
			}),
		)
		expect(partial.diskAtLeast).toBe(true)
		expect(partial.disk).toMatchObject({ used: 1024, percent: 2 })
	})

	it('marks exactly the dimension the server names', () => {
		const gauges = gaugesFor(
			me({
				limits: ordinary,
				allocatedMib: 1024,
				memoryLimitMib: 4096,
				diskUsedBytes: 60000 * MIB,
				diskLimitMib: 51200,
				over: ['disk'],
			}),
		)

		expect(gauges.disk.over).toBe(true)
		expect(gauges.memory.over).toBe(false)
	})

	it('clamps an overdrawn share at a hundred instead of running past it', () => {
		const gauges = gaugesFor(
			me({
				limits: ordinary,
				allocatedMib: 12288,
				memoryLimitMib: 4096,
				diskUsedBytes: 200000 * MIB,
				diskLimitMib: 51200,
				over: ['memory', 'disk'],
			}),
		)

		expect(gauges.memory.percent).toBe(100)
		expect(gauges.disk.percent).toBe(100)
		expect(gauges.memory.used).toBe(12288)
	})

	it('does not divide by zero', () => {
		const gauges = gaugesFor(
			me({
				limits: { ...ordinary, memory_mib: 0, disk_mib: 0 },
				allocatedMib: 512,
				memoryLimitMib: 0,
				diskUsedBytes: 0,
				diskLimitMib: 0,
			}),
		)

		expect(gauges.memory.percent).toBe(100)
		expect(gauges.disk.percent).toBe(0)
	})
})
