import { GenericModrinthClient, type RequestOptions } from '@modrinth/api-client'
import {
	AbstractWebNotificationManager,
	type NotificationPanelLocation,
	provideModalBehavior,
	provideModrinthClient,
	provideNotificationManager,
	providePageContext,
	type WebNotification,
} from '@modrinth/ui'
import { ref } from 'vue'

import { API_BASE, MODRINTH_PROXY_BASE, type PowerAction } from '@/api'

const CLIENT_ROOT = ''
const PANEL_VERSION = API_BASE.replace(/^\//, '')

const POWER_PATH = /^\/servers\/[^/]+\/power$/
const POWER_ACTIONS = new Map<string, PowerAction>([
	['Start', 'start'],
	['Stop', 'stop'],
	['Restart', 'restart'],
	['Kill', 'kill'],
])

function powerActionOf(body: unknown): PowerAction | null {
	if (typeof body !== 'object' || body === null || !('action' in body)) return null
	const { action } = body
	return typeof action === 'string' ? (POWER_ACTIONS.get(action) ?? null) : null
}

class PanelNotifications extends AbstractWebNotificationManager {
	private readonly items = ref<WebNotification[]>([])
	private readonly location = ref<NotificationPanelLocation>('right')

	getNotifications(): WebNotification[] {
		return this.items.value
	}

	getNotificationLocation(): NotificationPanelLocation {
		return this.location.value
	}

	setNotificationLocation(location: NotificationPanelLocation): void {
		this.location.value = location
	}

	protected addNotificationToStorage(notification: WebNotification): void {
		this.items.value.push(notification)
	}

	protected removeNotificationFromStorage(id: string | number): void {
		this.items.value = this.items.value.filter((item) => item.id !== id)
	}

	protected removeNotificationFromStorageByIndex(index: number): void {
		this.items.value.splice(index, 1)
	}

	protected clearAllNotificationsFromStorage(): void {
		this.items.value = []
	}
}

class PanelModrinthClient extends GenericModrinthClient {
	async request<T>(path: string, options: RequestOptions): Promise<T> {
		if (options.api !== 'archon') return super.request<T>(path, options)

		if (options.method === 'POST' && POWER_PATH.test(path)) {
			const action = powerActionOf(options.body)
			if (action !== null) {
				return super.request<T>(path, { ...options, version: PANEL_VERSION, body: { action } })
			}
		}

		throw new Error(`No panel route for ${options.method ?? 'GET'} ${path} (9.18)`)
	}
}

export function provideNotifications(): AbstractWebNotificationManager {
	const notifications = new PanelNotifications()
	provideNotificationManager(notifications)
	return notifications
}

export function provideModrinthHost(): void {
	provideModrinthClient(
		new PanelModrinthClient({
			archonBaseUrl: CLIENT_ROOT,
			labrinthBaseUrl: MODRINTH_PROXY_BASE,
		}),
	)

	providePageContext({
		hierarchicalSidebarAvailable: ref(false),
		showAds: ref(false),
		adConsentAvailable: ref(false),
		openExternalUrl: (url) => {
			window.open(url, '_blank', 'noopener,noreferrer')
		},
	})

	provideModalBehavior({ noblur: ref(false) })
}
