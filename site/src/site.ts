import { en, type Dict } from './i18n/en'
import { ja } from './i18n/ja'
import { ko } from './i18n/ko'

export const SITE_URL = 'https://tolkmeet.com'
/** 앱(회의). 로그인·가입·결제는 모두 여기서 한다. 소개 페이지는 쿠키를 쓰지 않는다. */
export const APP_URL = 'https://meet.tolkmeet.com'
export const SIGN_IN_URL = `${APP_URL}/login`
// 가입·결제는 아직 앱에 없다. 이 소개 페이지는 /signup과 Paddle 결제가 생긴 뒤에 공개한다(docs/LAUNCH_PLAN.md).
export const SIGNUP_URL = `${APP_URL}/signup`
// 레포 이름을 parley → tolkmeet로 바꾼 뒤의 주소(GitHub은 옛 주소를 새 주소로 넘겨 준다).
export const GITHUB_URL = 'https://github.com/sogangCSmaster/tolkmeet'

/** 언어 선택에 보이는 순서 */
export const LOCALES: Dict[] = [en, ko, ja]
