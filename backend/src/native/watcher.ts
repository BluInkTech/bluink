// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { watch } from 'node:fs'
import type { BluinkApplication } from '../../runtime/index.js'

type DirectorySubscription = {
	on(event: 'error', listener: () => void): unknown
	close(): void
}

type WatchDirectory = (
	directory: string,
	options: { recursive: false },
	listener: (event: string, filename: string | Buffer | null) => void,
) => DirectorySubscription

/** Only OS subscription and event transport live here. Rust interprets paths. */
export function subscribeProject(
	application: Pick<
		BluinkApplication,
		'notifyFileChange' | 'reportWatcherError'
	>,
	roots: Array<{ path: string; prefix: string }>,
	watchDirectory: WatchDirectory = watch,
): () => void {
	const subscriptions: DirectorySubscription[] = []
	let stopped = false
	for (const root of roots) {
		try {
			const subscription = watchDirectory(
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
		if (stopped) return
		stopped = true
		for (const subscription of subscriptions) subscription.close()
	}
}
