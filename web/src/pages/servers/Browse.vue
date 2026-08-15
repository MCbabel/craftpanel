<template>
	<div v-if="loading" class="relative grid place-items-center py-16">
		<LoadingIndicator />
	</div>

	<div v-else-if="browse.loadError.value" class="flex justify-center py-16">
		<ErrorInformationCard
			:title="formatMessage(messages.catalogueFailedTitle)"
			:description="formatMessage(messages.catalogueFailedText)"
			:icon="TriangleAlertIcon"
			:action="{
				label: formatMessage(commonMessages.retryButton),
				onClick: () => void browse.load(),
				icon: UpdatedIcon,
			}"
			:error-details="[
				{
					label: formatMessage(commonMessages.errorLabel),
					value: browse.loadError.value.message,
					type: 'inline',
				},
			]"
		/>
	</div>

	<template v-else>
		<BrowseInstallHeader divider />

		<Admonition
			v-if="browse.searchError.value"
			type="critical"
			:header="formatMessage(messages.searchFailedTitle)"
			:body="browse.searchError.value.message"
		>
			<template #actions>
				<Button type="colored" color="red" @click="browse.refreshSearch()">
					<UpdatedIcon />
					{{ formatMessage(commonMessages.retryButton) }}
				</Button>
			</template>
		</Admonition>

		<div class="grid min-w-0 gap-3 lg:grid-cols-[18.75rem_minmax(0,1fr)]">
			<aside class="min-w-0" :aria-label="formatMessage(commonMessages.filtersLabel)">
				<BrowseSidebar />
			</aside>

			<section class="flex min-w-0 flex-col gap-3">
				<BrowsePageLayout>
					<template #display-mode-icon>
						<GridIcon v-if="browse.displayMode.value === 'grid'" />
						<ImageIcon v-else-if="browse.displayMode.value === 'gallery'" />
						<ListIcon v-else />
					</template>
				</BrowsePageLayout>
			</section>
		</div>

		<SelectedProjectsFloatingBar />
	</template>
</template>

<script setup lang="ts">
import { GridIcon, ImageIcon, ListIcon, TriangleAlertIcon, UpdatedIcon } from '@modrinth/assets'
import {
	Admonition,
	BrowseInstallHeader,
	BrowsePageLayout,
	BrowseSidebar,
	Button,
	commonMessages,
	defineMessages,
	ErrorInformationCard,
	LoadingIndicator,
	SelectedProjectsFloatingBar,
	useVIntl,
} from '@modrinth/ui'
import { computed } from 'vue'

import { useServerPage } from '@/composables/server-page'
import { useBrowseManager } from '@/providers/browse-manager'

const messages = defineMessages({
	catalogueFailedTitle: {
		id: 'craftpanel.browse.catalogue-failed-title',
		defaultMessage: 'The catalogue could not be opened',
	},
	catalogueFailedText: {
		id: 'craftpanel.browse.catalogue-failed-text',
		defaultMessage:
			'Modrinth is fetched by the panel, not by your browser. This machine did not reach it.',
	},
	searchFailedTitle: {
		id: 'craftpanel.browse.search-failed-title',
		defaultMessage: 'The search failed',
	},
})

const { formatMessage } = useVIntl()
const page = useServerPage()

const browse = useBrowseManager({
	serverId: page.serverId,
	socket: page.socket,
	busyReasons: page.context.busyReasons,
	back: { name: 'server-content', params: { id: page.serverId } },
})

const loading = computed(() => !browse.ready.value && browse.loadError.value === null)
</script>
