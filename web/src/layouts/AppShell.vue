<template>
	<div class="flex min-h-full flex-col">
		<header class="border-0 border-b border-solid border-divider bg-bg-raised">
			<div class="mx-auto flex h-16 w-full max-w-[1280px] items-center gap-4 px-6">
				<RouterLink
					:to="{ name: 'servers' }"
					class="flex items-center gap-2 text-lg font-extrabold text-contrast no-underline"
				>
					<ServerStackIcon class="size-6 text-brand" />
					CraftPanel
				</RouterLink>

				<TeleportOverflowMenu
					class="ml-auto !gap-1 !rounded-xl !px-2"
					type="quiet"
					size="lg"
					interaction="none"
					:icon-only="false"
					:label="username"
					:options="options"
				>
					<Avatar :src="user?.avatar_url" size="1.75rem" circle />
					<span class="max-sm:hidden">{{ username }}</span>
					<DropdownIcon class="size-5 text-secondary" />
				</TeleportOverflowMenu>
			</div>
		</header>

		<main class="mx-auto w-full max-w-[1280px] flex-1 px-6 py-6">
			<RouterView />
		</main>

		<NewModal
			ref="themeModal"
			:header="formatMessage(commonSettingsMessages.appearance)"
			width="44rem"
		>
			<ThemeSelector
				:update-color-theme="setTheme"
				:current-theme="choice"
				:theme-options="themeChoices"
				:system-theme-color="systemTheme"
			/>
		</NewModal>

		<NewModal
			ref="languageModal"
			:header="formatMessage(commonSettingsMessages.language)"
			width="44rem"
		>
			<LanguageSelector
				:current-locale="locale"
				:locales="locales"
				:on-locale-change="changeLocale"
			/>
		</NewModal>

		<NotificationPanel />
	</div>
</template>

<script setup lang="ts">
import {
	CircleUserIcon,
	DropdownIcon,
	KeyIcon,
	LanguagesIcon,
	LogOutIcon,
	PaletteIcon,
	ServerStackIcon,
} from '@modrinth/assets'
import {
	Avatar,
	commonMessages,
	commonSettingsMessages,
	defineMessages,
	LanguageSelector,
	LOCALES,
	NewModal,
	NotificationPanel,
	type OverflowMenuOption,
	TeleportOverflowMenu,
	ThemeSelector,
	useVIntl,
} from '@modrinth/ui'
import { computed, onScopeDispose, ref } from 'vue'
import { RouterLink, RouterView, useRouter } from 'vue-router'

import { setCopyFailureHandler } from '@/clipboard'
import { isLocale, useLocale } from '@/composables/locale'
import { provideNotifications } from '@/composables/modrinth-host'
import { useSession } from '@/composables/session'
import { themeChoices, useTheme } from '@/composables/theme'
import { adminPages } from '@/pages/admin/routes'

const { formatMessage } = useVIntl()
const router = useRouter()
const { user, isAdmin, signOut } = useSession()
const { choice, systemTheme, setTheme } = useTheme()
const { locale, setLocale } = useLocale()

const notifications = provideNotifications()

const themeModal = ref<InstanceType<typeof NewModal> | null>(null)
const languageModal = ref<InstanceType<typeof NewModal> | null>(null)

const locales = LOCALES.filter((entry) => isLocale(entry.code))

function changeLocale(next: string): Promise<void> {
	setLocale(next)
	return Promise.resolve()
}

const messages = defineMessages({
	account: {
		id: 'panel.menu.account',
		defaultMessage: 'Your account',
	},
	changePassword: {
		id: 'panel.menu.change-password',
		defaultMessage: 'Change password',
	},
	copyFailed: {
		id: 'panel.copy.failed',
		defaultMessage: 'Nothing was copied',
	},
	copyFailedHint: {
		id: 'panel.copy.failed-hint',
		defaultMessage:
			'This browser would not put the text on the clipboard. Select it and copy it by hand.',
	},
})

setCopyFailureHandler(() =>
	notifications.addNotification({
		title: formatMessage(messages.copyFailed),
		text: formatMessage(messages.copyFailedHint),
		type: 'error',
	}),
)
onScopeDispose(() => setCopyFailureHandler(null))

const username = computed(() => user.value?.username ?? '')

const options = computed<OverflowMenuOption[]>(() => [
	{
		id: 'servers',
		type: 'link',
		to: { name: 'servers' },
		label: formatMessage(commonMessages.serversLabel),
		icon: ServerStackIcon,
	},
	{
		id: 'account',
		type: 'link',
		to: { name: 'account' },
		label: formatMessage(messages.account),
		icon: CircleUserIcon,
	},
	{
		id: 'change-password',
		type: 'link',
		to: { name: 'change-password' },
		label: formatMessage(messages.changePassword),
		icon: KeyIcon,
	},
	{
		id: 'appearance',
		label: formatMessage(commonSettingsMessages.appearance),
		icon: PaletteIcon,
		action: () => themeModal.value?.show(),
	},
	{
		id: 'language',
		label: formatMessage(commonSettingsMessages.language),
		icon: LanguagesIcon,
		action: () => languageModal.value?.show(),
	},
	{ type: 'divider', shown: isAdmin.value },
	...adminPages.flatMap((page): OverflowMenuOption[] =>
		page.label === null
			? []
			: [
					{
						id: page.name,
						type: 'link',
						to: { name: page.name },
						label: formatMessage(page.label),
						icon: page.icon ?? undefined,
						shown: isAdmin.value,
					},
				],
	),
	{ type: 'divider' },
	{
		id: 'sign-out',
		label: formatMessage(commonMessages.signOutButton),
		icon: LogOutIcon,
		tone: 'red',
		hoverFilled: true,
		action: () => void leave(),
	},
])

async function leave() {
	await signOut()
	await router.push({ name: 'login' })
}
</script>
