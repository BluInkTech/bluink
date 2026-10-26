import assert from 'node:assert/strict'
import { test } from 'node:test'
import { explainPortInUse } from './port-in-use.js'

test('explains a taken port with the command that frees it on each platform', () => {
	const windows = explainPortInUse(
		new Error(
			'Only one usage of each socket address (protocol/network address/port) is normally permitted. (os error 10048)',
		),
		9000,
		'win32',
	)
	assert.ok(windows instanceof Error)
	assert.match(windows.message, /^Port 9000 is already in use/)
	assert.match(
		windows.message,
		/In PowerShell, run:\n {2}Get-NetTCPConnection -LocalPort 9000 -State Listen \| ForEach-Object \{ Stop-Process -Id \$_\.OwningProcess \}/,
	)
	for (const [message, platform] of [
		['Address already in use (os error 98)', 'linux'],
		['Address already in use (os error 48)', 'darwin'],
	] as const) {
		const explained = explainPortInUse(new Error(message), 9100, platform)
		assert.ok(explained instanceof Error)
		assert.match(
			explained.message,
			/lsof -ti tcp:9100 -sTCP:LISTEN \| xargs kill/,
		)
	}
})

test('leaves other failures unchanged', () => {
	const other = new Error('Permission denied (os error 13)')
	assert.equal(explainPortInUse(other, 9000, 'win32'), other)
})
