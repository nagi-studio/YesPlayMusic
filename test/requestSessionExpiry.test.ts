import { beforeEach, expect, mock, test } from 'bun:test';
import { handleNcmSessionExpiry } from '../src/utils/sessionExpiry';

const logout = mock(() => undefined);
const navigate = mock((_route: 'login' | 'loginAccount') => undefined);

beforeEach(() => {
  logout.mockClear();
  navigate.mockClear();
});

test('网易云 301 会清除桌面会话并跳回账号登录页', () => {
  expect(
    handleNcmSessionExpiry(
      { code: 301, msg: '需要登录' },
      { loginRoute: 'loginAccount', logout, navigate }
    )
  ).toBe(true);

  expect(logout).toHaveBeenCalledTimes(1);
  expect(navigate).toHaveBeenCalledWith('loginAccount');
});

test('其他上游错误不会误清本地登录态', () => {
  expect(
    handleNcmSessionExpiry(
      { code: 301, msg: '临时错误' },
      { loginRoute: 'loginAccount', logout, navigate }
    )
  ).toBe(false);

  expect(logout).not.toHaveBeenCalled();
  expect(navigate).not.toHaveBeenCalled();
});
