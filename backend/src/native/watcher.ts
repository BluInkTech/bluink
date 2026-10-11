// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { watch, type FSWatcher } from 'node:fs'
import type { BluinkApplication } from '../../runtime/index.js'

/** Only OS subscription and event transport live here. Rust interprets paths. */
export function subscribeProject(
	application: Pick<
		BluinkApplication,
		'notifyFileChange' | 'reportWatcherError'
	>,
	roots: Array<{ path: string; prefix: string }>,
): () => void {
	const subscriptions: FSWatcher[] = []
	let stopped = false
	for (const root of roots) {
		try {
			const subscription = watch(
				root.path,
				// Deep external edits are reconciled by Rust's periodic stamp scan.
				// Recursive Linux subscriptions allocate a handle per directory.
				{ recursive: false },
				(_event, filename) => {
					if (stopped) return
					try {
						application.notifyFileChange(
							filename ? `${root.prefix}/${filename.toString()}` : undefined,
						)
					} catch {
						application.reportWatcherError()
					}
				},
			)
			subscription.on('error', () => {
				if (!stopped) application.reportWatcherError()
			})
			subscriptions.push(subscription)
		} catch {
			application.reportWatcherError()
		}
	}
	return () => {
		stopped = true
		for (const subscription of subscriptions) subscription.close()
	}
}
