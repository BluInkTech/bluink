// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { readFile, writeFile } from 'node:fs/promises'
import { createServer } from 'node:net'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import { seedPages, withTempDir } from '../test-utils/fixtures.js'
import { nativeAddon } from './addon.js'

const cli = fileURLToPath(new URL('../../bluink', import.meta.url))
const nativePath = fileURLToPath(
	new URL('../../runtime/index.cjs', import.meta.url),
)

function launch(root: string, args: string[], preload?: string) {
	const child = spawn(
		process.execPath,
		[...(preload ? ['--import', preload] : []), cli, ...args],
		{
			cwd: root,
			env: {
				...process.env,
				BLUINK_LOG_LEVEL: 'debug',
				BLUINK_CACHE_LOCATION: 'project',
				BLUINK_DIAGNOSTICS_FILE: path.join(root, 'diagnostics.jsonl'),
				NO_COLOR: '1',
			},
			stdio: ['ignore', 'pipe', 'pipe'],
		},
	)
	let stdout = ''
	let stderr = ''
	child.stdout.on('data', (data) => {
		stdout += data.toString()
	})
	child.stderr.on('data', (data) => {
		stderr += data.toString()
	})
	const exited = once(child, 'exit')
	return {
		child,
		exited,
		get stdout() {
			return stdout
		},
		get stderr() {
			return stderr
		},
		async ready() {
			for (let attempt = 0; attempt < 500; attempt++) {
				const match = `${stdout}\n${stderr}`.match(
					/Server started at (http:\/\/127\.0\.0\.1:\d+)/,
				)
				if (match) return match[1]!
				if (child.exitCode !== null || child.signalCode !== null) {
					throw new Error(`CLI exited before listening: ${stdout}\n${stderr}`)
				}
				await new Promise((resolve) => setTimeout(resolve, 20))
			}
			throw new Error(`CLI did not start: ${stdout}\n${stderr}`)
		},
		async dispose() {
			if (child.exitCode === null && child.signalCode === null) {
				child.kill('SIGKILL')
				await exited
			}
		},
	}
}

async function reopen(root: string) {
	const application = new (nativeAddon().BluinkApplication)({
		projectRoot: root,
		timezone: 'UTC',
		cacheLocation: 'project',
	})
	try {
		await application.initialize(true)
	} finally {
		await application.stop()
	}
}

test('compiled CLI help, version and argument errors work without loading the native backend', {
	timeout: 20_000,
}, async () => {
	await withTempDir(async (directory) => {
		const preload = path.join(directory, 'missing-native.mjs')
		await writeFile(
			preload,
			`
import { createRequire } from 'node:module';
const Module = createRequire(import.meta.url)('node:module');
Module._extensions['.node'] = () => { throw new Error('Injected native-load failure'); };
`,
		)
		for (const command of ['server', 'export', 'publication']) {
			const running = launch(directory, [command, '--help'], preload)
			try {
				assert.deepEqual(await running.exited, [0, null], running.stderr)
				assert.match(running.stdout, new RegExp(`Usage: .* ${command}`))
				assert.equal(running.stderr, '')
				assert.equal(running.stdout.includes('\u001b'), false)
			} finally {
				await running.dispose()
			}
		}
		const version = launch(directory, ['--version'], preload)
		try {
			assert.deepEqual(await version.exited, [0, null], version.stderr)
			assert.match(version.stdout, /^@bluink\/cli v\S+\n$/)
			assert.equal(version.stderr, '')
		} finally {
			await version.dispose()
		}
		const invalid = launch(directory, ['unknown-command'], preload)
		try {
			assert.deepEqual(await invalid.exited, [1, null], invalid.stderr)
			assert.match(invalid.stderr, /Unknown command: 'unknown-command'/)
			assert.match(invalid.stdout, /Usage:/)
			assert.doesNotMatch(invalid.stderr, /native backend/)
		} finally {
			await invalid.dispose()
		}
	})
})

test('compiled CLI reports native-load failure without entering terminal mode', {
	timeout: 20_000,
}, async () => {
	await withTempDir(async (directory) => {
		const preload = path.join(directory, 'missing-native.mjs')
		await writeFile(
			preload,
			`
import { createRequire } from 'node:module';
const Module = createRequire(import.meta.url)('node:module');
Module._extensions['.node'] = () => { throw new Error('Injected native-load failure'); };
`,
		)
		const running = launch(directory, ['server', '--port', '0'], preload)
		try {
			assert.deepEqual(await running.exited, [1, null], running.stderr)
			assert.equal(running.stdout, '')
			assert.match(running.stderr, /native backend could not be loaded/)
			assert.match(running.stderr, /Reinstall Bluink/)
			assert.equal(running.stderr.includes('\u001b'), false)
		} finally {
			await running.dispose()
		}
	})
})

test('compiled CLI plain mode displays real Rust startup counters without terminal escapes', {
	timeout: 20_000,
}, async () => {
	await withTempDir(async (directory) => {
		await seedPages(
			new (nativeAddon().BluinkApplication)({
				projectRoot: directory,
				timezone: 'UTC',
				cacheLocation: 'project',
			}),
			[{ title: 'Progress fixture' }],
		)
		const running = launch(directory, ['server', '--port', '0', '--no-tui'])
		try {
			await running.ready()
			assert.equal(running.stdout, '')
			assert.match(running.stderr, /1\/1 \(100%\)/)
			assert.equal(running.stderr.includes('\u001b'), false)
			running.child.kill('SIGTERM')
			assert.deepEqual(await running.exited, [0, null], running.stderr)
		} finally {
			await running.dispose()
		}
	})
})

test('compiled CLI remains alive, coalesces signals and flushes before releasing resources', {
	timeout: 20_000,
}, async () => {
	await withTempDir(async (directory) => {
		const running = launch(directory, ['server', '--port', '0'])
		try {
			const url = await running.ready()
			await new Promise((resolve) => setTimeout(resolve, 200))
			assert.equal(running.child.exitCode, null)
			const response = await fetch(`${url}/api/cache_status`)
			assert.equal(response.status, 200)
			await response.arrayBuffer()
			running.child.kill('SIGTERM')
			running.child.kill('SIGINT')
			assert.deepEqual(await running.exited, [0, null], running.stderr)
			assert.equal(
				(
					`${running.stdout}\n${running.stderr}`.match(/shutdown_requested/g) ??
					[]
				).length,
				1,
			)
			const diagnostics = await readFile(
				path.join(directory, 'diagnostics.jsonl'),
				'utf8',
			)
			assert.match(diagnostics, /cache_status/)
			assert.match(
				diagnostics,
				/shutdown_requested/,
				'Node host events must reach the native diagnostic sink',
			)
			assert.ok(
				diagnostics
					.trim()
					.split('\n')
					.some((line) => JSON.parse(line).name === 'application.stop'),
				'Final Rust application cleanup must reach capture before CLI exit',
			)
			await reopen(directory)
		} finally {
			await running.dispose()
		}
	})
})

test('compiled CLI cleans partial startup after bind and project initialization failures', {
	timeout: 20_000,
}, async () => {
	await withTempDir(async (directory) => {
		const blocker = createServer()
		blocker.listen(0, '127.0.0.1')
		await once(blocker, 'listening')
		const address = blocker.address()
		assert.ok(address && typeof address !== 'string')
		const collision = launch(directory, [
			'server',
			'--port',
			String(address.port),
		])
		try {
			assert.deepEqual(await collision.exited, [1, null], collision.stderr)
			assert.match(collision.stderr, /address|Address/)
			await reopen(directory)
		} finally {
			await collision.dispose()
			await new Promise<void>((resolve, reject) =>
				blocker.close((error) => (error ? reject(error) : resolve())),
			)
		}
		const invalidProject = path.join(directory, 'not-a-folder')
		await writeFile(invalidProject, 'not a project directory')
		const invalid = launch(directory, [
			'server',
			'--project',
			invalidProject,
			'--port',
			'0',
		])
		try {
			assert.deepEqual(await invalid.exited, [1, null], invalid.stderr)
			assert.doesNotMatch(
				`${invalid.stdout}\n${invalid.stderr}`,
				/Server started/,
			)
			assert.ok(invalid.stderr.length > 0)
		} finally {
			await invalid.dispose()
		}
	})
})

test('compiled CLI registers stop before native initialization is awaited', {
	timeout: 20_000,
}, async () => {
	await withTempDir(async (directory) => {
		const preload = path.join(directory, 'early-stop.mjs')
		// Observe the real native boundary and emit the registered host signal
		// synchronously while its initialization promise is still pending.
		await writeFile(
			preload,
			`
import { createRequire } from 'node:module';
const { BluinkApplication } = createRequire(import.meta.url)(${JSON.stringify(nativePath)});
const initialize = BluinkApplication.prototype.initialize;
BluinkApplication.prototype.initialize = function (...args) {
  const pending = initialize.apply(this, args);
  process.stderr.write('INITIALIZATION_PENDING\\n');
  process.emit('SIGTERM');
  process.emit('SIGINT');
  return pending;
};
`,
		)
		const early = launch(directory, ['server', '--port', '0'], preload)
		try {
			assert.deepEqual(await early.exited, [0, null], early.stderr)
			assert.match(early.stderr, /INITIALIZATION_PENDING/)
			assert.doesNotMatch(`${early.stdout}\n${early.stderr}`, /Server started/)
			await reopen(directory)
		} finally {
			await early.dispose()
		}
	})
})

test('compiled export command shares early-stop coordination without polluting stdout', {
	timeout: 20_000,
}, async () => {
	await withTempDir(async (directory) => {
		const [page] = await seedPages(
			new (nativeAddon().BluinkApplication)({
				projectRoot: directory,
				timezone: 'UTC',
				cacheLocation: 'project',
			}),
			[{ title: 'Interrupted command fixture' }],
		)
		const id = page!.id
		const preload = path.join(directory, 'command-stop.mjs')
		await writeFile(
			preload,
			`
import { createRequire } from 'node:module';
const { BluinkApplication } = createRequire(import.meta.url)(${JSON.stringify(nativePath)});
const execute = BluinkApplication.prototype.executeCommand;
BluinkApplication.prototype.executeCommand = function (...args) {
  const pending = execute.apply(this, args);
  process.stderr.write('COMMAND_PENDING\\n');
  process.emit('SIGTERM');
  process.emit('SIGINT');
  return pending;
};
`,
		)
		const command = launch(
			directory,
			['export', 'html', '--note', id, '--check'],
			preload,
		)
		try {
			assert.deepEqual(await command.exited, [0, null], command.stderr)
			assert.match(command.stderr, /COMMAND_PENDING/)
			assert.doesNotMatch(command.stdout, /Performing cleanup/)
			if (command.stdout) assert.equal(JSON.parse(command.stdout).pageId, id)
			await reopen(directory)
		} finally {
			await command.dispose()
		}
	})
})

test('compiled CLI propagates cleanup failure after draining native resources', {
	timeout: 20_000,
}, async () => {
	await withTempDir(async (directory) => {
		const preload = path.join(directory, 'failed-stop.mjs')
		await writeFile(
			preload,
			`
import { createRequire } from 'node:module';
const { BluinkApplication } = createRequire(import.meta.url)(${JSON.stringify(nativePath)});
const stop = BluinkApplication.prototype.stop;
BluinkApplication.prototype.stop = async function (...args) {
  await stop.apply(this, args);
  process.stderr.write('NATIVE_STOP_FINISHED\\n');
  throw new Error('Injected cleanup failure');
};
`,
		)
		const running = launch(directory, ['server', '--port', '0'], preload)
		try {
			await running.ready()
			running.child.kill('SIGTERM')
			assert.deepEqual(await running.exited, [1, null], running.stderr)
			assert.match(running.stderr, /Backend cleanup failed/)
			assert.equal(
				(running.stderr.match(/NATIVE_STOP_FINISHED/g) ?? []).length,
				1,
			)
			await reopen(directory)
		} finally {
			await running.dispose()
		}
	})
})
