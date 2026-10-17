import { useState } from 'react'
import { openPortal } from '../lib/api'
import { useI18n } from '../lib/i18n'

/**
 * 이번 기간의 결제가 아직 끝나지 않아(갱신 결제 실패) 포함 시간이 없을 때. 결제 수단을 고치면 Paddle이 다시 청구하고,
 * 결제가 끝나면 서버가 이번 기간의 시간을 준다. 홈·계정의 사용량 카드에 쓴다(회의 중 대화상자는 새 탭으로 연다).
 */
export function UnpaidNotice() {
  const { t, locale } = useI18n()
  const [busy, setBusy] = useState(false)
  const [failed, setFailed] = useState(false)
  return (
    <div className="form-error" role="status">
      {/* 일본어는 문장 사이를 띄우지 않는다. */}
      {t('billing.unpaid')}
      {locale === 'ja' ? '' : ' '}
      {t('billing.unpaidBody')}{' '}
      <button
        className="link-btn"
        disabled={busy}
        onClick={() => {
          setBusy(true)
          setFailed(false)
          void openPortal().then((ok) => {
            setBusy(false)
            setFailed(!ok)
          })
        }}
      >
        {t('checkout.updatePayment')}
      </button>
      {failed && <span role="alert"> {t('account.billingFailed')}</span>}
    </div>
  )
}
