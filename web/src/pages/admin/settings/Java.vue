<template>
	<Card class="!mb-0 flex flex-col gap-4">
		<div class="flex items-center justify-between gap-4">
			<label class="flex flex-col gap-0.5" for="java-auto-install">
				<span class="text-lg font-extrabold text-contrast">
					{{ formatMessage(messages.title) }}
				</span>
				<span class="text-sm text-secondary">
					{{ formatMessage(messages.description) }}
				</span>
			</label>
			<Toggle id="java-auto-install" v-model="settings.java_auto_install" />
		</div>

		<Admonition
			v-if="!settings.java_auto_install"
			type="warning"
			:header="formatMessage(messages.offHeader)"
			:body="formatMessage(messages.offBody)"
		/>

		<p class="m-0 text-sm text-secondary">
			{{ formatMessage(messages.listHint) }}
			<RouterLink class="text-link hover:underline" :to="{ name: 'admin-runtimes' }">
				{{ formatMessage(messages.listLink) }}
			</RouterLink>
		</p>
	</Card>
</template>

<script setup lang="ts">
import { Admonition, Card, defineMessages, Toggle, useVIntl } from '@modrinth/ui'
import { RouterLink } from 'vue-router'

import type { PanelSettings } from '@/api'

const { formatMessage } = useVIntl()

const settings = defineModel<PanelSettings>({ required: true })

const messages = defineMessages({
	title: { id: 'admin.settings.java.title', defaultMessage: 'Fetch Java by itself' },
	description: {
		id: 'admin.settings.java.description',
		defaultMessage:
			'When a server needs a Java version this machine does not have, the panel downloads that runtime from Adoptium and keeps it in its own data directory.',
	},
	offHeader: {
		id: 'admin.settings.java.off-header',
		defaultMessage: 'You provide the runtimes yourself now',
	},
	offBody: {
		id: 'admin.settings.java.off-body',
		defaultMessage:
			'A server whose Java version is missing then refuses to start and says which one it wanted, instead of the panel going and getting it. Everything already downloaded stays and keeps working. Turn this off on a machine without outbound access, or when you install your JVMs with the package manager.',
	},
	listHint: {
		id: 'admin.settings.java.list-hint',
		defaultMessage: 'What is on this machine, and the buttons to add or remove one, are under',
	},
	listLink: { id: 'admin.settings.java.list-link', defaultMessage: 'Java runtimes' },
})
</script>
