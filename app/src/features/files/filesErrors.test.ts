import { filesErrors } from '../../i18n/zh/filesErrors';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { friendlyError } from './filesErrors';

it.each(Object.entries(filesErrors))(
  'maps %s to its Chinese message',
  (code, message) => {
    const friendly = friendlyError(
      new RsCoreError('http', `${code}: detail`, 404),
    );
    expect(friendly).toBeInstanceOf(RsCoreError);
    expect(friendly).toMatchObject({
      kind: 'http',
      httpStatus: 404,
      message,
    });
  },
);

it('keeps an error without a known code unchanged', () => {
  const error = new RsCoreError('network', 'network unavailable', 503);
  expect(friendlyError(error)).toBe(error);
});

it('keeps an error with an unknown files code unchanged', () => {
  const error = new RsCoreError('http', 'files.unknown: detail', 500);
  expect(friendlyError(error)).toBe(error);
});

it('keeps non-RsCore errors unchanged', () => {
  const error = new Error('files.notFound: detail');
  expect(friendlyError(error)).toBe(error);
  expect(friendlyError('plain')).toBe('plain');
});
