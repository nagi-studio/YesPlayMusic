export type LoginRouteName = 'login' | 'loginAccount';

export interface SessionExpiryActions {
  loginRoute: LoginRouteName;
  logout: () => void;
  navigate: (route: LoginRouteName) => void;
}

export function handleNcmSessionExpiry(
  data: unknown,
  actions: SessionExpiryActions
): boolean {
  if (
    typeof data !== 'object' ||
    data === null ||
    !('code' in data) ||
    data.code !== 301 ||
    !('msg' in data) ||
    data.msg !== '需要登录'
  ) {
    return false;
  }

  actions.logout();
  actions.navigate(actions.loginRoute);
  return true;
}
