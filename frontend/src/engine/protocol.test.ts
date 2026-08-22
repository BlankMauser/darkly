import { describe, expect, test, vi } from 'vitest';
import { Engine } from './protocol';

describe('Engine disposal', () => {
    test('cancels a scheduled drain before freeing its handle', async () => {
        const handle = {
            enqueue: vi.fn(),
            drain: vi.fn(() => ({ busy: false, results: [] })),
            free: vi.fn(),
        } as unknown as ConstructorParameters<typeof Engine>[0];
        const engine = new Engine(handle);

        const pending = engine.transport.request('begin_stroke', { id: 1 });
        engine.free();

        await expect(pending).rejects.toMatchObject({
            kind: 'engine_error',
            message: 'engine disposed',
        });
        await new Promise((resolve) => setTimeout(resolve, 0));
        expect(handle.drain).not.toHaveBeenCalled();
        expect(handle.free).toHaveBeenCalledOnce();
    });
});
