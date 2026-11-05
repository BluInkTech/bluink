// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import test from 'node:test'
import { hasMediaType } from './smoke-package.mjs'

test('installed-package HTTP smoke accepts a CSS charset parameter', () => {
	const css = new Response('body {}', {
		headers: { 'content-type': 'text/css; charset=utf-8' },
	})
	assert.equal(hasMediaType(css, 'text/css'), true)
	assert.equal(hasMediaType(css, 'text/javascript'), false)
	assert.equal(
		hasMediaType(
			new Response('', {
				status: 404,
				headers: { 'content-type': 'text/css' },
			}),
			'text/css',
		),
		false,
	)
})
