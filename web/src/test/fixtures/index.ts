import type { ResultOf } from '../../api/coreClient';
import type { CoreResponse } from '../../api/generated/core-response';
import { enclosureDescribe, plateDescribe, plateValidate, repeatedDescribe } from './core-fixtures';

export * from './core-fixtures';

function okResult<Op extends 'describe' | 'validate'>(
  response: CoreResponse,
  op: Op,
): ResultOf<Op> {
  if (response.status !== 'ok' || response.result.op !== op) {
    throw new Error(`fixture is not an ok ${op} response`);
  }
  return response.result as ResultOf<Op>;
}

export const plateDescription = () => okResult(plateDescribe, 'describe');
export const enclosureDescription = () => okResult(enclosureDescribe, 'describe');
export const repeatedDescription = () => okResult(repeatedDescribe, 'describe');
export const plateValidation = () => okResult(plateValidate, 'validate');
