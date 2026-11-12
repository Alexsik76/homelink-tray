# HomeLink

[English](README.md)

Програма для системного лотка Windows, яка відкриває доступ до домашнього ПК одним
кліком: підіймає тунель WireGuard, доручає маршрутизатору MikroTik надіслати
Wake-on-LAN і показує стан тунелю та ПК у значку.

## Як це працює

- **Лівий клік** по значку: підняти тунель, перевірити порти ПК і, якщо він
  мовчить, надіслати Wake-on-LAN. Коли ПК відповість, з'явиться сповіщення.
- **Правий клік**: підключити чи відключити тунель, розбудити ПК, відкрити RDP,
  відкрити файл налаштувань, вийти.
- **Значок**: колір екрана показує стан ПК (зелений: у мережі, бурштиновий:
  пробуджується, сірий: вимкнений), кружок показує стан тунелю (синій: підключено,
  жовтий: з'єднання, червоний: вимкнено).

Магічний пакет надсилає маршрутизатор, а не цей комп'ютер: широкомовний кадр не
проходить крізь маршрутизований тунель. Програма звертається до REST API
маршрутизатора (`POST /rest/tool/wol`) через тунель.

## Вимоги

- Windows 10 або 11, права адміністратора (тунель працює як служба Windows).
- Офіційний [клієнт WireGuard](https://www.wireguard.com/install/), у який один раз
  імпортовано тунель.
- MikroTik RouterOS 7 з REST API, доступним через тунель.
- Python 3.11+ для збирання чи запуску з вихідного коду.

## Налаштування маршрутизатора

Створіть користувача, якому дозволено лише надсилати Wake-on-LAN і лише з тунельної
адреси цього комп'ютера:

```
/user group add name=wol policy=read,write,test,rest-api,api
/user add name=homelink group=wol address=<тунельна адреса цього комп'ютера>/32 password=<пароль>
/ip service set www disabled=no
```

Політики `test` вимагає команда `/tool wol`; політика `api` перевіряється під час
входу через REST.

## Збирання

```
build.bat
```

Результат: `dist\HomeLink.exe`. Перед повторним збиранням закрийте запущену програму.

## Налаштування

Під час першого запуску програма створює поруч із виконуваним файлом `config.json`
із вбудованого шаблону, повідомляє його розташування й завершується. Заповніть файл і
запустіть програму знову. Усі ключі обов'язкові: типових значень у коді немає.

| Ключ | Значення |
| --- | --- |
| `language` | Мова інтерфейсу: `uk` або `en` (файл у `locales/`) |
| `wireguard_exe` | Шлях до `wireguard.exe` |
| `tunnel_conf` | Шлях до імпортованого тунелю: `...\Data\Configurations\<назва>.conf.dpapi` |
| `tunnel_timeout` | Скільки секунд чекати відповіді маршрутизатора після запуску тунелю |
| `router_ip` | Адреса маршрутизатора в тунелі, за нею перевіряється тунель |
| `router_api` | Базова адреса REST API, `http://` або `https://` |
| `router_user`, `router_password` | Користувач маршрутизатора; пароль зберігається відкритим текстом |
| `router_verify_tls` | Чи перевіряти сертифікат маршрутизатора для `https://` |
| `router_timeout` | Скільки секунд чекати відповіді маршрутизатора |
| `wol_mac` | MAC-адреса ПК |
| `wol_interface` | Інтерфейс маршрутизатора, звернений до ПК |
| `pc_ip` | Адреса ПК |
| `pc_ports` | Порти TCP, які всі мають приймати з'єднання, щоб ПК вважався доступним, наприклад `[3389]` для RDP |
| `port_timeout_ms` | Час очікування однієї перевірки порту |
| `offline_after_failures` | Скільки невдалих перевірок поспіль потрібно, щоб доступний ПК показати вимкненим; для стану «у мережі» достатньо однієї вдалої |
| `ping_timeout_ms` | Час очікування одного ping маршрутизатора |
| `poll_seconds` | Інтервал між перевірками стану |
| `wake_poll_seconds` | Той самий інтервал під час пробудження ПК |
| `wake_timeout` | Скільки секунд чекати ПК після Wake-on-LAN |
| `rdp_user` | Ім'я користувача у файлі RDP |
| `rdp_file` | Назва файлу RDP відносно `config.json` |
| `rdp_options` | Додаткові рядки файлу RDP |
| `icons_dir` | Тека з власними значками; порожній рядок означає вбудований набір |
| `log_file` | Назва файлу журналу відносно `config.json` |
| `log_level` | Докладність журналу: `DEBUG`, `INFO`, `WARNING` або `ERROR` |

Файл RDP створюється під час першого виклику «Відкрити RDP» і надалі не
перезаписується, тому його можна підписати. Щоб застосувати змінені параметри
`rdp_*`, видаліть файл.

## Автозапуск без запиту UAC

У PowerShell від імені адміністратора:

```
schtasks /create /tn HomeLink /tr "C:\шлях\до\HomeLink.exe" /sc onlogon /rl highest /f
```

## Віддалений робочий стіл

- **Збережений пароль.** Збережіть його без передавання в рядку команди:
  `cmdkey /generic:TERMSRV/<pc_ip> /user:<користувач> /pass`
- **Попередження «Unknown remote connection».** Підпишіть файл RDP і додайте видавця
  до довірених (PowerShell від імені адміністратора):

```powershell
$rdp = "C:\шлях\до\home.rdp"
$cert = New-SelfSignedCertificate -Subject "CN=HomeLink RDP" -Type CodeSigningCert `
    -CertStoreLocation Cert:\LocalMachine\My -NotAfter (Get-Date).AddYears(10)
Export-Certificate -Cert $cert -FilePath "$env:TEMP\homelink.cer" | Out-Null
Import-Certificate -FilePath "$env:TEMP\homelink.cer" -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
rdpsign.exe /v /sha256 $cert.Thumbprint $rdp
reg add "HKLM\Software\Policies\Microsoft\Windows NT\Terminal Services" `
    /v TrustedCertThumbprints /t REG_SZ /d $cert.Thumbprint /f
```

## Власні значки

Набір значків складається з дванадцяти файлів PNG з назвами
`<стан ПК>_<стан тунелю>.png`. Стан ПК: `unknown`, `offline`, `waking` або `online`;
стан тунелю: `down`, `starting` або `up`. Покладіть файли в теку й укажіть її в
`icons_dir`. Вбудований набір створює команда `python tools/generate_icons.py`.

## Розробка

```
py -m venv .venv
.venv\Scripts\python -m pip install -e ".[dev]"
.venv\Scripts\python -m pytest
.venv\Scripts\python -m ruff check .
.venv\Scripts\python -m ruff format .
.venv\Scripts\pythonw run_homelink.pyw
```

| Модуль | Відповідальність |
| --- | --- |
| `config.py` | Читання й перевірка `config.json` |
| `state.py` | Стани тунелю та ПК |
| `controller.py` | Логіка: дії та стеження за станом, без інтерфейсу |
| `tunnel.py` | Служба тунелю WireGuard |
| `router.py` | Wake-on-LAN через REST API MikroTik |
| `net.py` | Ping і перевірка портів |
| `rdp.py` | Файл RDP |
| `icons.py`, `i18n.py` | Значки та рядки інтерфейсу |
| `app.py` | Значок у лотку й меню |
| `elevation.py` | Права адміністратора |

## Ліцензія

[MIT](LICENSE)
