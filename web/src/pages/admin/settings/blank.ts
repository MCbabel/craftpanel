import type { PanelSettings } from '@/api'

const MIB = 1024 * 1024

export function blankSettings(): PanelSettings {
	return {
		public_address: null,
		port_pool: { from: 25565, to: 25665 },
		default_limits: {
			memory_mib: 2048,
			cpu_mode: 'share',
			cpu_cores: 2,
			pids_max: 512,
			disk_mib: 51200,
		},
		max_upload_bytes: 512 * MIB,
		max_backups_per_server: 10,
		external_services_enabled: true,
		max_concurrent_operations: 4,
		stop_grace_seconds: 60,
		registration_enabled: false,
		registration_requires_approval: true,
	}
}
