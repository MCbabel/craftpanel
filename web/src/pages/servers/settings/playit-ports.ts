import type { ServerTunnel } from '@/api/playit'

export function tunnelHoldsPrimaryPort(tunnel: ServerTunnel | null, available: boolean): boolean {
	return available && tunnel !== null && tunnel.state !== 'none'
}

export function publishedPort(tunnel: ServerTunnel | null, primaryPort: number): number {
	return tunnel?.local_port ?? primaryPort
}
