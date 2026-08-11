<template>
  <div class="settings-page" @click="clickOutside">
    <div class="container">
      <div v-if="showUserInfo" class="user">
        <div class="left">
          <img class="avatar" :src="data.user.avatarUrl" loading="lazy" />
          <div class="info">
            <div class="nickname">{{ data.user.nickname }}</div>
            <div class="extra-info">
              <span v-if="data.user.vipType !== 0" class="vip"
                ><img
                  class="cvip"
                  src="data:image/png;base64,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"
                  loading="lazy"
                />
                <span class="text">{{ $t('settings.vinylVip') }}</span>
              </span>
              <span v-else class="text">{{ data.user.signature }}</span>
            </div>
          </div>
        </div>
        <div class="right">
          <button @click="logout">
            <svg-icon icon-class="logout" />
            {{ $t('settings.logout') }}
          </button>
        </div>
      </div>

      <div class="item">
        <div class="left">
          <div class="title"> {{ $t('settings.language') }} </div>
        </div>
        <div class="right">
          <select v-model="lang">
            <option
              v-for="option in localeOptions"
              :key="option.code"
              :value="option.code"
            >
              {{ option.label }}
            </option>
          </select>
        </div>
      </div>
      <div class="item">
        <div class="left">
          <div class="title"> {{ $t('settings.appearance.text') }} </div>
        </div>
        <div class="right">
          <select v-model="appearance">
            <option value="auto">{{ $t('settings.appearance.auto') }}</option>
            <option value="light"
              >🌞 {{ $t('settings.appearance.light') }}</option
            >
            <option value="dark"
              >🌚 {{ $t('settings.appearance.dark') }}</option
            >
          </select>
        </div>
      </div>
      <div class="item">
        <div class="left">
          <div class="title"> {{ $t('settings.themeColor.text') }} </div>
        </div>
        <div class="right">
          <select v-model="themeColor">
            <option value="default">
              {{ $t('settings.themeColor.default') }}
            </option>
            <option value="sunset">
              {{ $t('settings.themeColor.sunset') }}
            </option>
            <option value="ocean">
              {{ $t('settings.themeColor.ocean') }}
            </option>
            <option value="forest">
              {{ $t('settings.themeColor.forest') }}
            </option>
          </select>
        </div>
      </div>
      <div v-if="isDesktop" class="item">
        <div class="left">
          <div class="title"> {{ $t('settings.trayIcon.text') }} </div>
        </div>
        <div class="right">
          <select v-model="trayIconTheme">
            <option value="auto">{{ $t('settings.trayIcon.auto') }}</option>
            <option value="light">{{ $t('settings.trayIcon.light') }}</option>
            <option value="dark">{{ $t('settings.trayIcon.dark') }}</option>
          </select>
        </div>
      </div>
      <div class="item">
        <div class="left">
          <div class="title">
            {{ $t('settings.MusicGenrePreference.text') }}
          </div>
        </div>
        <div class="right">
          <select v-model="musicLanguage">
            <option value="all">{{
              $t('settings.MusicGenrePreference.none')
            }}</option>
            <option value="zh">{{
              $t('settings.MusicGenrePreference.mandarin')
            }}</option>
            <option value="ea">{{
              $t('settings.MusicGenrePreference.western')
            }}</option>
            <option value="jp">{{
              $t('settings.MusicGenrePreference.japanese')
            }}</option>
            <option value="kr">{{
              $t('settings.MusicGenrePreference.korean')
            }}</option>
          </select>
        </div>
      </div>

      <div class="item">
        <div class="left">
          <div class="title"> {{ $t('settings.musicQuality.text') }} </div>
        </div>
        <div class="right">
          <select v-model="musicQuality">
            <option :value="128000">
              {{ $t('settings.musicQuality.low') }} - 128Kbps
            </option>
            <option :value="192000">
              {{ $t('settings.musicQuality.medium') }} - 192Kbps
            </option>
            <option :value="320000">
              {{ $t('settings.musicQuality.high') }} - 320Kbps
            </option>
            <option value="flac">
              {{ $t('settings.musicQuality.lossless') }} - FLAC
            </option>
            <option :value="999000">Hi-Res</option>
          </select>
        </div>
      </div>
      <div v-if="isDesktop" class="item">
        <div class="left">
          <div class="title"> {{ $t('settings.deviceSelector') }} </div>
        </div>
        <div class="right">
          <select v-model="outputDevice">
            <option
              v-for="device in allOutputDevices"
              :key="device.deviceId"
              :value="device.deviceId"
              :selected="device.deviceId == outputDevice"
            >
              {{
                device.deviceId === 'default' &&
                device.label === 'settings.permissionRequired'
                  ? $t('settings.permissionRequired')
                  : device.label
              }}
            </option>
          </select>
        </div>
      </div>

      <h3 v-if="isDesktop">{{ $t('settings.cache') }}</h3>
      <div v-if="isDesktop" class="item">
        <div class="left">
          <div class="title">
            {{ $t('settings.automaticallyCacheSongs') }}
          </div>
        </div>
        <div class="right">
          <div class="toggle">
            <input
              id="automatically-cache-songs"
              v-model="automaticallyCacheSongs"
              type="checkbox"
              name="automatically-cache-songs"
            />
            <label for="automatically-cache-songs"></label>
          </div>
        </div>
      </div>
      <div v-if="isDesktop" class="item">
        <div class="left">
          <div class="title"> {{ $t('settings.cacheLimit.text') }} </div>
        </div>
        <div class="right">
          <select v-model="cacheLimit">
            <option :value="null">
              {{ $t('settings.cacheLimit.none') }}
            </option>
            <option :value="512"> 500MB </option>
            <option :value="1024"> 1GB </option>
            <option :value="2048"> 2GB </option>
            <option :value="4096"> 4GB </option>
            <option :value="8192"> 8GB </option>
          </select>
        </div>
      </div>
      <div v-if="isDesktop" class="item">
        <div class="left">
          <div class="title">
            {{
              $t('settings.cacheCount', {
                song: tracksCache.length,
                size: tracksCache.size,
              })
            }}</div
          >
        </div>
        <div class="right">
          <button @click="clearCache()">
            {{ $t('settings.clearSongsCache') }}
          </button>
        </div>
      </div>

      <h3>{{ $t('settings.lyric') }}</h3>
      <div class="item">
        <div class="left">
          <div class="title">{{ $t('settings.showLyricsTranslation') }}</div>
        </div>
        <div class="right">
          <div class="toggle">
            <input
              id="show-lyrics-translation"
              v-model="showLyricsTranslation"
              type="checkbox"
              name="show-lyrics-translation"
            />
            <label for="show-lyrics-translation"></label>
          </div>
        </div>
      </div>
      <div class="item">
        <div class="left">
          <div class="title">{{ $t('settings.lyricsBackground.text') }}</div>
        </div>
        <div class="right">
          <select v-model="lyricsBackground">
            <option :value="false">
              {{ $t('settings.lyricsBackground.off') }}
            </option>
            <option :value="true">
              {{ $t('settings.lyricsBackground.on') }}
            </option>
            <option value="blur">
              {{ $t('settings.lyricsBackground.blur') }}
            </option>
            <option value="dynamic">
              {{ $t('settings.lyricsBackground.dynamic') }}
            </option>
          </select>
        </div>
      </div>
      <div class="item">
        <div class="left">
          <div class="title"> {{ $t('settings.showLyricsTime') }} </div>
        </div>
        <div class="right">
          <div class="toggle">
            <input
              id="show-lyrics-time"
              v-model="showLyricsTime"
              type="checkbox"
              name="show-lyrics-time"
            />
            <label for="show-lyrics-time"></label>
          </div>
        </div>
      </div>
      <div class="item">
        <div class="left">
          <div class="title"> {{ $t('settings.lyricFontSize.text') }} </div>
        </div>
        <div class="right">
          <select v-model="lyricFontSize">
            <option :value="16">
              {{ $t('settings.lyricFontSize.small') }} - 16px
            </option>
            <option :value="22">
              {{ $t('settings.lyricFontSize.medium') }} - 22px
            </option>
            <option :value="28">
              {{ $t('settings.lyricFontSize.large') }} - 28px
            </option>
            <option :value="36">
              {{ $t('settings.lyricFontSize.xlarge') }} - 36px
            </option>
          </select>
        </div>
      </div>
      <section v-if="isDesktop" class="unm-configuration">
        <h3>UnblockNeteaseMusic</h3>
        <div class="item">
          <div class="left">
            <div class="title"
              >{{ $t('settings.unm.enable') }}
              <a
                href="https://github.com/UnblockNeteaseMusic/server"
                target="blank"
                >UnblockNeteaseMusic</a
              ></div
            >
          </div>
          <div class="right">
            <div class="toggle">
              <input
                id="enable-unblock-netease-music"
                v-model="enableUnblockNeteaseMusic"
                type="checkbox"
                name="enable-unblock-netease-music"
              />
              <label for="enable-unblock-netease-music"></label>
            </div>
          </div>
        </div>

        <div class="item">
          <div class="left">
            <div class="title">
              {{ $t('settings.unm.audioSource.title') }}
            </div>
            <div class="description">
              {{ $t('settings.unm.audioSource.desc1') }}
              <a
                href="https://github.com/UnblockNeteaseMusic/server-rust/blob/main/README.md#支援的所有引擎"
                target="_blank"
              >
                {{ $t('settings.unm.audioSource.desc2') }} </a
              ><br />
              {{ $t('settings.unm.audioSource.desc3') }}<br />
              {{ $t('settings.unm.audioSource.desc4') }}
            </div>
          </div>
          <div class="right">
            <input
              v-model="unmSource"
              class="text-input margin-right-0"
              :placeholder="$t('settings.unm.audioSource.placeholder')"
            />
          </div>
        </div>

        <div class="item">
          <div class="left">
            <div class="title"> {{ $t('settings.unm.enableFlac.title') }} </div>
            <div class="description">
              {{ $t('settings.unm.enableFlac.desc') }}
            </div>
          </div>
          <div class="right">
            <div class="toggle">
              <input
                id="unm-enable-flac"
                v-model="unmEnableFlac"
                type="checkbox"
              />
              <label for="unm-enable-flac" />
            </div>
          </div>
        </div>

        <div class="item">
          <div class="left">
            <div class="title"> {{ $t('settings.unm.searchMode.title') }} </div>
          </div>
          <div class="right">
            <select v-model="unmSearchMode">
              <option value="fast-first">
                {{ $t('settings.unm.searchMode.fast') }}
              </option>
              <option value="order-first">
                {{ $t('settings.unm.searchMode.order') }}
              </option>
            </select>
          </div>
        </div>

        <div class="item">
          <div class="left">
            <div class="title">{{ $t('settings.unm.cookie.joox') }}</div>
            <div class="description">
              <a
                href="https://github.com/UnblockNeteaseMusic/server-rust/tree/main/engines#joox-cookie-設定說明"
                target="_blank"
                >{{ $t('settings.unm.cookie.desc1') }}
              </a>
              {{ $t('settings.unm.cookie.desc2') }}
            </div>
          </div>
          <div class="right">
            <input
              v-model="unmJooxCookie"
              class="text-input margin-right-0"
              placeholder="wmid=..; session_key=.."
            />
          </div>
        </div>

        <div class="item">
          <div class="left">
            <div class="title"> {{ $t('settings.unm.cookie.qq') }} </div>
            <div class="description">
              <a
                href="https://github.com/UnblockNeteaseMusic/server-rust/tree/main/engines#qq-cookie-設定說明"
                target="_blank"
                >{{ $t('settings.unm.cookie.desc1') }}
              </a>
              {{ $t('settings.unm.cookie.desc2') }}
            </div>
          </div>
          <div class="right">
            <input
              v-model="unmQQCookie"
              class="text-input margin-right-0"
              placeholder="uin=..; qm_keyst=..;"
            />
          </div>
        </div>

        <div class="item">
          <div class="left">
            <div class="title"> {{ $t('settings.unm.ytdl') }} </div>
            <div class="description">
              <a
                href="https://github.com/UnblockNeteaseMusic/server-rust/tree/main/engines#ytdlexe-設定說明"
                target="_blank"
                >{{ $t('settings.unm.cookie.desc1') }}
              </a>
              {{ $t('settings.unm.cookie.desc2') }}
            </div>
          </div>
          <div class="right">
            <input
              v-model="unmYtDlExe"
              class="text-input margin-right-0"
              placeholder="ex. youtube-dl"
            />
          </div>
        </div>

        <div class="item">
          <div class="left">
            <div class="title"> {{ $t('settings.unm.proxy.title') }} </div>
            <div class="description">
              {{ $t('settings.unm.proxy.desc1') }}<br />
              {{ $t('settings.unm.proxy.desc2') }}
            </div>
          </div>
          <div class="right">
            <input
              v-model="unmProxyUri"
              class="text-input margin-right-0"
              placeholder="ex. https://192.168.11.45"
            />
          </div>
        </div>
      </section>

      <h3>{{ $t('settings.customization') }}</h3>
      <div class="item">
        <div class="left">
          <div class="title">
            {{
              isLastfmConnected
                ? $t('settings.lastfm.connected', { name: lastfm.name })
                : $t('settings.lastfm.connect')
            }}</div
          >
        </div>
        <div class="right">
          <button v-if="isLastfmConnected" @click="lastfmDisconnect()"
            >{{ $t('settings.lastfm.disconnect') }}
          </button>
          <button v-else @click="lastfmConnect()">
            {{ $t('settings.lastfm.authorize') }}
          </button>
        </div>
      </div>
      <div v-if="isDesktop" class="item">
        <div class="left">
          <div class="title">
            {{ $t('settings.enableDiscordRichPresence') }}</div
          >
        </div>
        <div class="right">
          <div class="toggle">
            <input
              id="enable-discord-rich-presence"
              v-model="enableDiscordRichPresence"
              type="checkbox"
              name="enable-discord-rich-presence"
            />
            <label for="enable-discord-rich-presence"></label>
          </div>
        </div>
      </div>
      <h3>{{ $t('settings.others') }}</h3>
      <div v-if="isDesktop && !isMac" class="item">
        <div class="left">
          <div class="title"> {{ $t('settings.closeAppOption.text') }} </div>
        </div>
        <div class="right">
          <select v-model="closeAppOption">
            <option value="ask">
              {{ $t('settings.closeAppOption.ask') }}
            </option>
            <option value="exit">
              {{ $t('settings.closeAppOption.exit') }}
            </option>
            <option value="minimizeToTray">
              {{ $t('settings.closeAppOption.minimizeToTray') }}
            </option>
          </select>
        </div>
      </div>

      <div v-if="isDesktop && isLinux" class="item">
        <div class="left">
          <div class="title"> {{ $t('settings.enableCustomTitlebar') }} </div>
        </div>
        <div class="right">
          <div class="toggle">
            <input
              id="enable-custom-titlebar"
              v-model="enableCustomTitlebar"
              type="checkbox"
              name="enable-custom-titlebar"
            />
            <label for="enable-custom-titlebar"></label>
          </div>
        </div>
      </div>

      <div v-if="isDesktop && isLinux" class="item">
        <div class="left">
          <div class="title">
            <a href="https://github.com/osdlyrics/osdlyrics" target="_blank"
              >OSDLyrics</a
            >
            {{ $t('settings.enableOsdlyricsSupport.title') }}
          </div>
          <div class="description">
            {{ $t('settings.enableOsdlyricsSupport.desc') }}
          </div>
        </div>
        <div class="right">
          <div class="toggle">
            <input
              id="enable-osdlyrics-support"
              v-model="enableOsdlyricsSupport"
              type="checkbox"
              name="enable-osdlyrics-support"
            />
            <label for="enable-osdlyrics-support"></label>
          </div>
        </div>
      </div>

      <div v-if="isDesktop" class="item">
        <div class="left">
          <div class="title"> {{ $t('settings.showLibraryDefault') }}</div>
        </div>
        <div class="right">
          <div class="toggle">
            <input
              id="show-library-default"
              v-model="showLibraryDefault"
              type="checkbox"
              name="show-library-default"
            />
            <label for="show-library-default"></label>
          </div>
        </div>
      </div>

      <div class="item">
        <div class="left">
          <div class="title">
            {{ $t('settings.showPlaylistsByAppleMusic') }}</div
          >
        </div>
        <div class="right">
          <div class="toggle">
            <input
              id="show-playlists-by-apple-music"
              v-model="showPlaylistsByAppleMusic"
              type="checkbox"
              name="show-playlists-by-apple-music"
            />
            <label for="show-playlists-by-apple-music"></label>
          </div>
        </div>
      </div>

      <div class="item">
        <div class="left">
          <div class="title">{{ $t('settings.subTitleDefault') }}</div>
        </div>
        <div class="right">
          <div class="toggle">
            <input
              id="sub-title-default"
              v-model="subTitleDefault"
              type="checkbox"
              name="sub-title-default"
            />
            <label for="sub-title-default"></label>
          </div>
        </div>
      </div>

      <div class="item">
        <div class="left">
          <div class="title">{{ $t('settings.enableReversedMode') }}</div>
        </div>
        <div class="right">
          <div class="toggle">
            <input
              id="enable-reversed-mode"
              v-model="enableReversedMode"
              type="checkbox"
              name="enable-reversed-mode"
            />
            <label for="enable-reversed-mode"></label>
          </div>
        </div>
      </div>

      <div class="item">
        <div class="left">
          <div class="title" style="transform: scaleX(-1)">🐈️ 🏳️‍🌈</div>
        </div>
        <div class="right">
          <div class="toggle">
            <input
              id="nyancat-style"
              v-model="nyancatStyle"
              type="checkbox"
              name="nyancat-style"
            />
            <label for="nyancat-style"></label>
          </div>
        </div>
      </div>

      <div class="item">
        <div class="left">
          <div class="title">🎀 Anon</div>
        </div>
        <div class="right">
          <div class="toggle">
            <input
              id="anon-style"
              v-model="anonStyle"
              type="checkbox"
              name="anon-style"
            />
            <label for="anon-style"></label>
          </div>
        </div>
      </div>

      <div v-if="isDesktop">
        <h3>{{ $t('settings.proxy.title') }}</h3>
        <div class="item">
          <div class="left">
            <div class="title"> {{ $t('settings.proxy.protocol') }} </div>
          </div>
          <div class="right">
            <select v-model="proxyProtocol">
              <option value="noProxy">{{ $t('settings.proxy.off') }}</option>
              <option value="HTTP">{{ $t('settings.proxy.http') }}</option>
              <option value="HTTPS">{{ $t('settings.proxy.https') }}</option>
            </select>
          </div>
        </div>
        <div id="proxy-form" :class="{ disabled: proxyProtocol === 'noProxy' }">
          <input
            v-model="proxyServer"
            class="text-input"
            :placeholder="$t('settings.proxy.server')"
            :disabled="proxyProtocol === 'noProxy'"
          /><input
            v-model="proxyPort"
            class="text-input"
            :placeholder="$t('settings.proxy.port')"
            type="number"
            min="1"
            max="65535"
            :disabled="proxyProtocol === 'noProxy'"
          />
          <button @click="sendProxyConfig">
            {{ $t('settings.proxy.update') }}
          </button>
        </div>
      </div>
      <div v-if="isDesktop">
        <h3>Real IP</h3>
        <div class="item">
          <div class="left">
            <div class="title"> Real IP </div>
          </div>
          <div class="right">
            <div class="toggle">
              <input
                id="enable-real-ip"
                v-model="enableRealIP"
                type="checkbox"
                name="enable-real-ip"
              />
              <label for="enable-real-ip"></label>
            </div>
          </div>
        </div>
        <div id="real-ip" :class="{ disabled: !enableRealIP }">
          <input
            v-model="realIP"
            class="text-input"
            :placeholder="$t('settings.realIPPlaceholder')"
            :disabled="!enableRealIP"
          />
        </div>
      </div>

      <div v-if="isDesktop">
        <h3>{{ $t('settings.shortcut.title') }}</h3>
        <div class="item">
          <div class="left">
            <div class="title"> {{ $t('settings.enableGlobalShortcut') }}</div>
          </div>
          <div class="right">
            <div class="toggle">
              <input
                id="enable-enable-global-shortcut"
                v-model="enableGlobalShortcut"
                type="checkbox"
                name="enable-enable-global-shortcut"
              />
              <label for="enable-enable-global-shortcut"></label>
            </div>
          </div>
        </div>
        <div
          id="shortcut-table"
          ref="shortcutTable"
          :class="{ 'global-disabled': !enableGlobalShortcut }"
          tabindex="0"
          @keydown="handleShortcutKeydown"
        >
          <div class="row row-head">
            <div class="col">{{ $t('settings.shortcut.function') }}</div>
            <div class="col">{{ $t('settings.shortcut.shortcut') }}</div>
            <div class="col">{{ $t('settings.shortcut.globalShortcut') }}</div>
          </div>
          <div
            v-for="shortcut in settings.shortcuts"
            :key="shortcut.id"
            class="row"
          >
            <div class="col">{{ shortcutName(shortcut) }}</div>
            <div class="col">
              <div
                class="keyboard-input"
                :class="{
                  active:
                    shortcutInput.id === shortcut.id &&
                    shortcutInput.type === 'shortcut',
                }"
                @click.stop="readyToRecordShortcut(shortcut.id, 'shortcut')"
              >
                {{
                  shortcutInput.id === shortcut.id &&
                  shortcutInput.type === 'shortcut' &&
                  recordedShortcutComputed !== ''
                    ? formatShortcut(recordedShortcutComputed)
                    : formatShortcut(shortcut.shortcut)
                }}
              </div>
            </div>
            <div class="col">
              <div
                class="keyboard-input"
                :class="{
                  active:
                    shortcutInput.id === shortcut.id &&
                    shortcutInput.type === 'globalShortcut' &&
                    enableGlobalShortcut,
                }"
                @click.stop="
                  readyToRecordShortcut(shortcut.id, 'globalShortcut')
                "
                >{{
                  shortcutInput.id === shortcut.id &&
                  shortcutInput.type === 'globalShortcut' &&
                  recordedShortcutComputed !== ''
                    ? formatShortcut(recordedShortcutComputed)
                    : formatShortcut(shortcut.globalShortcut)
                }}</div
              >
            </div>
          </div>
          <button
            class="restore-default-shortcut"
            @click="restoreDefaultShortcuts"
            >{{ $t('settings.shortcut.restoreDefault') }}</button
          >
        </div>
      </div>

      <template v-if="isDesktop">
        <h3>{{ $t('settings.updater.title') }}</h3>
        <div class="item updater">
          <div class="left">
            <div class="title">
              {{ $t('settings.updater.currentVersion', { version }) }}
            </div>
            <div class="description">{{ updaterStatusText }}</div>
            <div v-if="updaterNotes" class="description updater-notes">
              {{ updaterNotes }}
            </div>
          </div>
          <div class="right">
            <button :disabled="updaterActionDisabled" @click="handleAppUpdate">
              {{ updaterActionText }}
            </button>
          </div>
        </div>
      </template>

      <div class="footer">
        <p class="author"
          >MADE BY
          <a href="http://github.com/qier222" target="_blank">QIER222</a></p
        >
        <p class="version">v{{ version }}</p>

        <a
          v-if="!isDesktop"
          href="https://vercel.com/?utm_source=ohmusic&utm_campaign=oss"
        >
          <img
            height="36"
            src="https://www.datocms-assets.com/31049/1618983297-powered-by-vercel.svg"
          />
        </a>
      </div>
    </div>
  </div>
</template>

<script lang="ts">
import { defineComponent } from 'vue';
import { mapActions, mapState } from 'pinia';
import { useAppStore } from '@/stores/app';
import { isLooseLoggedIn, doLogout } from '@/utils/auth';
import { auth as lastfmAuth } from '@/api/lastfm';
import {
  persistAuthorizedLastfmSession,
  startDesktopLastfmAuthorization,
} from '@/services/lastfmAuth';
import {
  changeAppearance,
  changeThemeColor,
  bytesToSize,
} from '@/utils/common';
import {
  clearTrackSourceCache,
  countDBSize,
  trimTrackSourceCache,
} from '@/utils/db';
import { normalizeCacheLimit } from '@/utils/cachePolicy';
import { stopInterval } from '@/utils/mediaLifecycle';
import {
  isLinux as platformIsLinux,
  isMac as platformIsMac,
} from '@/utils/platform';
import pkg from '../../package.json';
import locale from '@/locale';
import { LOCALE_OPTIONS, normalizePersistedLocale } from '@/locale/catalog';
import type { LocaleCode } from '@/locale/catalog';
import { relaunchDesktop, sendDesktop } from '@/services/desktopTransport';
import { isDesktopRuntime } from '@/utils/runtime';
import { getRecordedShortcutKeyIdentity } from '@/utils/shortcuts';
import type { RecordedShortcutKey } from '@/utils/shortcuts';
import {
  decodeLastfmState,
  normalizeLyricFontSize,
  normalizeMusicQuality,
  readStoredJson,
} from '@/utils/persistedState';
import type { SettingsState } from '@/types/persistence';
import {
  checkForAppUpdate,
  clearPendingAppUpdate,
  installPendingAppUpdate,
} from '@/services/appUpdater';
import { syncDesktopSettings } from '@/services/desktopSettings';

// Only these locales spell out the space bar; English keeps the key name.
const SPACE_KEY_LABELS: Partial<Record<LocaleCode, string>> = {
  ja: 'スペース',
  'zh-CN': '空格',
  'zh-TW': '空白鍵',
};

const SHORTCUT_NAME_KEYS = Object.freeze({
  play: 'settings.shortcut.actions.play',
  next: 'settings.shortcut.actions.next',
  previous: 'settings.shortcut.actions.previous',
  increaseVolume: 'settings.shortcut.actions.increaseVolume',
  decreaseVolume: 'settings.shortcut.actions.decreaseVolume',
  like: 'settings.shortcut.actions.like',
  minimize: 'settings.shortcut.actions.minimize',
} as const);

const validShortcutCodes = ['=', '-', '~', '[', ']', ';', "'", ',', '.', '/'];

type ShortcutKind = 'shortcut' | 'globalShortcut';
type AppUpdaterState =
  | 'idle'
  | 'checking'
  | 'available'
  | 'downloading'
  | 'unconfigured'
  | 'upToDate'
  | 'error';

interface OutputDevice {
  deviceId: string;
  label: string;
}

export default defineComponent({
  name: 'Settings',
  data() {
    return {
      localeOptions: LOCALE_OPTIONS,
      tracksCache: {
        size: '0KB',
        length: 0,
      },
      allOutputDevices: [
        {
          deviceId: 'default',
          label: 'settings.permissionRequired',
        },
      ] as OutputDevice[],
      shortcutInput: {
        id: '',
        type: '' as ShortcutKind | '',
        recording: false,
      },
      recordedShortcut: [] as RecordedShortcutKey[],
      lastfmChecker: null as ReturnType<typeof setInterval> | null,
      lastfmAuthorizationCleanup: null as (() => void) | null,
      lastfmAuthorizationEpoch: 0,
      updaterState: 'idle' as AppUpdaterState,
      updaterVersion: '',
      updaterNotes: '',
      updaterProgress: null as number | null,
    };
  },
  computed: {
    ...mapState(useAppStore, ['player', 'settings', 'data', 'lastfm']),
    isDesktop() {
      return isDesktopRuntime;
    },
    isMac() {
      return platformIsMac;
    },
    isLinux() {
      return platformIsLinux;
    },
    version() {
      return pkg.version;
    },
    updaterActionDisabled(): boolean {
      return ['checking', 'downloading', 'unconfigured'].includes(
        this.updaterState
      );
    },
    updaterActionText(): string {
      if (this.updaterState === 'checking') {
        return String(this.$t('settings.updater.checking'));
      }
      if (this.updaterState === 'downloading') {
        return this.updaterProgress === null
          ? String(this.$t('settings.updater.downloading'))
          : String(
              this.$t('settings.updater.downloadingProgress', {
                progress: this.updaterProgress,
              })
            );
      }
      if (this.updaterState === 'available') {
        return String(this.$t('settings.updater.install'));
      }
      return String(this.$t('settings.updater.check'));
    },
    updaterStatusText(): string {
      if (this.updaterState === 'unconfigured') {
        return String(this.$t('settings.updater.unconfigured'));
      }
      if (this.updaterState === 'upToDate') {
        return String(this.$t('settings.updater.upToDate'));
      }
      if (this.updaterState === 'available') {
        return String(
          this.$t('settings.updater.available', {
            version: this.updaterVersion,
          })
        );
      }
      if (this.updaterState === 'downloading') {
        return String(this.$t('settings.updater.installing'));
      }
      if (this.updaterState === 'error') {
        return String(this.$t('settings.updater.failed'));
      }
      return String(this.$t('settings.updater.ready'));
    },
    showUserInfo() {
      return isLooseLoggedIn() && this.data.user.nickname;
    },
    recordedShortcutComputed(): string {
      let shortcut: string[] = [];
      this.recordedShortcut.forEach(event => {
        if (/^Key[A-Z]$/.test(event.code)) {
          // A-Z
          shortcut.push(event.code.replace('Key', ''));
        } else if (event.key === 'Meta') {
          // ⌘ Command on macOS
          shortcut.push('Command');
        } else if (['Alt', 'Control', 'Shift'].includes(event.key)) {
          shortcut.push(event.key);
        } else if (/^Digit[0-9]$/.test(event.code)) {
          // 0-9
          shortcut.push(event.code.replace('Digit', ''));
        } else if (/^F(?:[1-9]|1[0-2])$/.test(event.code)) {
          // F1-F12
          shortcut.push(event.code);
        } else if (event.code === 'Space') {
          shortcut.push('Space');
        } else if (
          ['ArrowRight', 'ArrowLeft', 'ArrowUp', 'ArrowDown'].includes(
            event.key
          )
        ) {
          // Arrows
          shortcut.push(event.code.replace('Arrow', ''));
        } else if (validShortcutCodes.includes(event.key)) {
          shortcut.push(event.key);
        }
      });
      const sortTable: Record<string, number> = {
        Control: 1,
        Shift: 2,
        Alt: 3,
        Command: 4,
      };
      shortcut = shortcut.sort((a, b) => {
        const aOrder = sortTable[a];
        const bOrder = sortTable[b];
        if (aOrder === undefined || bOrder === undefined) return 0;
        if (aOrder - bOrder <= -1) {
          return -1;
        } else if (aOrder - bOrder >= 1) {
          return 1;
        } else {
          return 0;
        }
      });
      return shortcut.join('+');
    },

    lang: {
      get() {
        // A retired locale left in localStorage must not leave the picker blank.
        return normalizePersistedLocale(this.settings.lang);
      },
      set(lang: LocaleCode) {
        this.$i18n.locale = lang;
        this.changeLang(lang);
      },
    },
    musicLanguage: {
      get() {
        return this.settings.musicLanguage ?? 'all';
      },
      set(value: SettingsState['musicLanguage']) {
        this.updateSettings({
          key: 'musicLanguage',
          value,
        });
      },
    },
    appearance: {
      get() {
        if (this.settings.appearance === undefined) return 'auto';
        return this.settings.appearance;
      },
      set(value: SettingsState['appearance']) {
        this.updateSettings({
          key: 'appearance',
          value,
        });
        changeAppearance(value);
        const resolvedAppearance =
          value === 'auto'
            ? document.body?.getAttribute('data-theme') || 'light'
            : value;
        changeThemeColor(this.themeColor, resolvedAppearance);
      },
    },
    themeColor: {
      get() {
        if (this.settings.themeColor === undefined) return 'default';
        return this.settings.themeColor;
      },
      set(value: SettingsState['themeColor']) {
        this.updateSettings({
          key: 'themeColor',
          value,
        });
        const resolvedAppearance =
          this.settings.appearance === 'auto'
            ? document.body?.getAttribute('data-theme') || 'light'
            : this.settings.appearance;
        changeThemeColor(value, resolvedAppearance);
      },
    },
    trayIconTheme: {
      get() {
        return this.settings.trayIconTheme;
      },
      set(value: SettingsState['trayIconTheme']) {
        this.updateSettings({ key: 'trayIconTheme', value });
      },
    },
    musicQuality: {
      get() {
        return this.settings.musicQuality ?? 320000;
      },
      set(value: unknown) {
        const normalized = normalizeMusicQuality(
          value,
          this.settings.musicQuality
        );
        if (normalized === this.settings.musicQuality) return;
        this.changeMusicQuality(normalized);
        this.clearCache();
      },
    },
    lyricFontSize: {
      get() {
        if (this.settings.lyricFontSize === undefined) return 28;
        return this.settings.lyricFontSize;
      },
      set(value: unknown) {
        this.changeLyricFontSize(
          normalizeLyricFontSize(value, this.settings.lyricFontSize)
        );
      },
    },
    outputDevice: {
      get() {
        const isValidDevice = this.allOutputDevices.find(
          device => device.deviceId === this.settings.outputDevice
        );
        if (
          this.settings.outputDevice === undefined ||
          isValidDevice === undefined
        )
          return 'default'; // Default deviceId
        return this.settings.outputDevice;
      },
      set(deviceId: string) {
        if (deviceId === this.settings.outputDevice || deviceId === undefined)
          return;
        this.changeOutputDevice(deviceId);
        this.player.setOutputDevice();
      },
    },
    enableUnblockNeteaseMusic: {
      get() {
        const value = this.settings.enableUnblockNeteaseMusic;
        return value !== undefined ? value : true;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'enableUnblockNeteaseMusic',
          value,
        });
      },
    },
    showPlaylistsByAppleMusic: {
      get() {
        if (this.settings.showPlaylistsByAppleMusic === undefined) return true;
        return this.settings.showPlaylistsByAppleMusic;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'showPlaylistsByAppleMusic',
          value,
        });
      },
    },
    nyancatStyle: {
      get() {
        if (this.settings.nyancatStyle === undefined) return false;
        return this.settings.nyancatStyle;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'nyancatStyle',
          value,
        });
        // Keep progress styles mutually exclusive.
        if (value) {
          this.updateSettings({
            key: 'anonStyle',
            value: false,
          });
        }
      },
    },
    anonStyle: {
      get() {
        if (this.settings.anonStyle === undefined) return false;
        return this.settings.anonStyle;
      },
      set(value: boolean) {
        this.updateSettings({ key: 'anonStyle', value });
        if (value) {
          this.updateSettings({
            key: 'nyancatStyle',
            value: false,
          });
        }
      },
    },
    automaticallyCacheSongs: {
      get() {
        if (this.settings.automaticallyCacheSongs === undefined) return false;
        return this.settings.automaticallyCacheSongs;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'automaticallyCacheSongs',
          value,
        });
      },
    },
    showLyricsTranslation: {
      get() {
        return this.settings.showLyricsTranslation;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'showLyricsTranslation',
          value,
        });
      },
    },
    lyricsBackground: {
      get() {
        return this.settings.lyricsBackground || false;
      },
      set(value: SettingsState['lyricsBackground']) {
        this.updateSettings({
          key: 'lyricsBackground',
          value,
        });
      },
    },
    showLyricsTime: {
      get() {
        return this.settings.showLyricsTime;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'showLyricsTime',
          value,
        });
      },
    },
    enableOsdlyricsSupport: {
      get() {
        return this.settings.enableOsdlyricsSupport;
      },
      set(value: boolean) {
        this.updateSettings({ key: 'enableOsdlyricsSupport', value });
        if (value) this.player.syncDesktopMediaMetadata();
      },
    },
    closeAppOption: {
      get() {
        return this.settings.closeAppOption;
      },
      set(value: SettingsState['closeAppOption']) {
        this.updateSettings({
          key: 'closeAppOption',
          value,
        });
      },
    },
    enableDiscordRichPresence: {
      get() {
        return this.settings.enableDiscordRichPresence;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'enableDiscordRichPresence',
          value,
        });
        if (value) this.player.syncDiscordPresence();
      },
    },
    subTitleDefault: {
      get() {
        return this.settings.subTitleDefault;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'subTitleDefault',
          value,
        });
      },
    },
    enableReversedMode: {
      get() {
        if (this.settings.enableReversedMode === undefined) return false;
        return this.settings.enableReversedMode;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'enableReversedMode',
          value,
        });
        if (value === false) {
          this.player.reversed = false;
        }
      },
    },
    enableGlobalShortcut: {
      get() {
        return this.settings.enableGlobalShortcut;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'enableGlobalShortcut',
          value,
        });
      },
    },
    showLibraryDefault: {
      get() {
        return this.settings.showLibraryDefault || false;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'showLibraryDefault',
          value,
        });
      },
    },
    cacheLimit: {
      get() {
        return normalizeCacheLimit(this.settings.cacheLimit);
      },
      set(value: number | null) {
        this.updateSettings({
          key: 'cacheLimit',
          value: normalizeCacheLimit(value),
        });
        trimTrackSourceCache().then(() => this.countDBSize());
      },
    },
    proxyProtocol: {
      get() {
        return this.settings.proxyConfig?.protocol || 'noProxy';
      },
      set(value: SettingsState['proxyConfig']['protocol']) {
        const config = { ...this.settings.proxyConfig };
        config.protocol = value;
        if (value === 'noProxy') {
          void this.disableProxy();
        }
        this.updateSettings({
          key: 'proxyConfig',
          value: config,
        });
      },
    },
    proxyServer: {
      get() {
        return this.settings.proxyConfig?.server || '';
      },
      set(value: string) {
        const config = { ...this.settings.proxyConfig };
        config.server = value;
        this.updateSettings({
          key: 'proxyConfig',
          value: config,
        });
      },
    },
    enableRealIP: {
      get() {
        return this.settings.enableRealIP || false;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'enableRealIP',
          value: value,
        });
      },
    },
    realIP: {
      get() {
        return this.settings.realIP || '';
      },
      set(value: SettingsState['proxyConfig']['protocol']) {
        this.updateSettings({
          key: 'realIP',
          value: value,
        });
      },
    },
    proxyPort: {
      get() {
        return this.settings.proxyConfig?.port || '';
      },
      set(value: string | number) {
        const config = { ...this.settings.proxyConfig };
        const port = Number(value);
        config.port = Number.isInteger(port) && port > 0 ? port : null;
        this.updateSettings({
          key: 'proxyConfig',
          value: config,
        });
      },
    },
    unmSource: {
      /**
       * @returns {string}
       */
      get() {
        return this.settings.unmSource || '';
      },
      /** @param {string?} value */
      set(value: string) {
        this.updateSettings({
          key: 'unmSource',
          value: value || undefined,
        });
      },
    },
    unmSearchMode: {
      get() {
        return this.settings.unmSearchMode || 'fast-first';
      },
      set(value: string) {
        this.updateSettings({
          key: 'unmSearchMode',
          value: value,
        });
      },
    },
    unmEnableFlac: {
      get() {
        return this.settings.unmEnableFlac || false;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'unmEnableFlac',
          value: value || false,
        });
      },
    },
    unmProxyUri: {
      get() {
        return this.settings.unmProxyUri || '';
      },
      set(value: string) {
        this.updateSettings({
          key: 'unmProxyUri',
          value: value || undefined,
        });
      },
    },
    unmJooxCookie: {
      get() {
        return this.settings.unmJooxCookie || '';
      },
      set(value: string) {
        this.updateSettings({
          key: 'unmJooxCookie',
          value: value || undefined,
        });
      },
    },
    unmQQCookie: {
      get() {
        return this.settings.unmQQCookie || '';
      },
      set(value: string) {
        this.updateSettings({
          key: 'unmQQCookie',
          value: value || undefined,
        });
      },
    },
    unmYtDlExe: {
      get() {
        return this.settings.unmYtDlExe || '';
      },
      set(value: string) {
        this.updateSettings({
          key: 'unmYtDlExe',
          value: value || undefined,
        });
      },
    },
    enableCustomTitlebar: {
      get() {
        return this.settings.linuxEnableCustomTitlebar;
      },
      set(value: boolean) {
        this.updateSettings({
          key: 'linuxEnableCustomTitlebar',
          value,
        });
      },
    },
    isLastfmConnected() {
      return this.lastfm['key'] !== undefined;
    },
  },
  created() {
    this.countDBSize();
    if (isDesktopRuntime) this.getAllOutputDevices();
  },
  activated() {
    this.countDBSize();
    if (isDesktopRuntime) this.getAllOutputDevices();
  },
  beforeUnmount() {
    this.stopLastfmChecker();
    this.stopLastfmAuthorization();
    void clearPendingAppUpdate();
  },
  methods: {
    shortcutName(shortcut: { id: string; name: string }): string {
      if (!Object.hasOwn(SHORTCUT_NAME_KEYS, shortcut.id)) return shortcut.name;
      return locale.t(
        SHORTCUT_NAME_KEYS[shortcut.id as keyof typeof SHORTCUT_NAME_KEYS]
      );
    },
    ...mapActions(useAppStore, [
      'showToast',
      'changeLang',
      'updateSettings',
      'changeMusicQuality',
      'changeLyricFontSize',
      'changeOutputDevice',
      'updateLastfm',
      'updateShortcut',
    ]),
    ...mapActions(useAppStore, {
      restoreDefaultShortcutsInStore: 'restoreDefaultShortcuts',
    }),
    getAllOutputDevices() {
      navigator.mediaDevices.enumerateDevices().then(devices => {
        this.allOutputDevices = devices
          .filter(device => device.kind === 'audiooutput')
          .map(({ deviceId, label }) => ({ deviceId, label }));
        if (
          this.allOutputDevices.length === 0 ||
          this.allOutputDevices[0]?.label === ''
        ) {
          this.allOutputDevices = [
            {
              deviceId: 'default',
              label: 'settings.permissionRequired',
            },
          ];
        }
      });
    },
    logout() {
      doLogout();
      this.$router.push({ name: 'home' });
    },
    countDBSize() {
      countDBSize().then(data => {
        if (data === undefined) {
          this.tracksCache = {
            size: '0KB',
            length: 0,
          };
          return;
        }
        this.tracksCache.size = bytesToSize(data.bytes);
        this.tracksCache.length = data.length;
      });
    },
    clearCache() {
      clearTrackSourceCache().then(() => {
        this.countDBSize();
      });
    },
    async handleAppUpdate() {
      if (this.updaterState === 'available') {
        await this.installAppUpdate();
        return;
      }
      await this.checkAppUpdate();
    },
    async checkAppUpdate() {
      this.updaterState = 'checking';
      this.updaterNotes = '';
      this.updaterProgress = null;
      try {
        const result = await checkForAppUpdate();
        if (result.status === 'unconfigured') {
          this.updaterState = 'unconfigured';
          return;
        }
        if (result.status === 'up-to-date') {
          this.updaterState = 'upToDate';
          this.showToast(String(this.$t('settings.updater.upToDate')));
          return;
        }
        this.updaterState = 'available';
        this.updaterVersion = result.version;
        this.updaterNotes = result.notes;
        this.showToast(
          String(
            this.$t('settings.updater.available', {
              version: result.version,
            })
          )
        );
      } catch (error) {
        console.error('[updater] update check failed', error);
        this.updaterState = 'error';
        this.showToast(String(this.$t('settings.updater.failed')));
      }
    },
    async installAppUpdate() {
      this.updaterState = 'downloading';
      this.updaterProgress = null;
      try {
        await installPendingAppUpdate(progress => {
          this.updaterProgress = progress.percent;
        });
      } catch (error) {
        console.error('[updater] update installation failed', error);
        this.updaterState = 'available';
        this.showToast(String(this.$t('settings.updater.failed')));
      }
    },
    lastfmConnect() {
      this.stopLastfmChecker();
      this.stopLastfmAuthorization();
      if (isDesktopRuntime) {
        const authorizationEpoch = this.lastfmAuthorizationEpoch;
        let completed = false;
        void startDesktopLastfmAuthorization({
          onAuthorized: session => {
            if (authorizationEpoch !== this.lastfmAuthorizationEpoch) return;
            completed = true;
            const persisted = persistAuthorizedLastfmSession(
              session,
              localStorage
            );
            this.updateLastfm(persisted);
            this.lastfmAuthorizationCleanup = null;
          },
          onError: error => {
            if (authorizationEpoch !== this.lastfmAuthorizationEpoch) return;
            console.error('[lastfm] authorization failed', error);
            this.showToast(locale.t('toast.lastfmAuthFailed'));
          },
        })
          .then(cleanup => {
            if (
              completed ||
              authorizationEpoch !== this.lastfmAuthorizationEpoch
            ) {
              cleanup();
              return;
            }
            this.lastfmAuthorizationCleanup = cleanup;
          })
          .catch(error => {
            if (authorizationEpoch !== this.lastfmAuthorizationEpoch) return;
            console.error(
              '[lastfm] failed to open authorization window',
              error
            );
            this.showToast(locale.t('toast.lastfmWindowFailed'));
          });
        return;
      }
      lastfmAuth();
      this.lastfmChecker = setInterval(() => {
        const session = localStorage.getItem('lastfm');
        if (session) {
          this.updateLastfm(
            decodeLastfmState(readStoredJson(localStorage, 'lastfm'))
          );
          this.stopLastfmChecker();
        }
      }, 1000);
    },
    lastfmDisconnect() {
      this.stopLastfmChecker();
      this.stopLastfmAuthorization();
      localStorage.removeItem('lastfm');
      this.updateLastfm({});
    },
    stopLastfmChecker() {
      stopInterval(this.lastfmChecker);
      this.lastfmChecker = null;
    },
    stopLastfmAuthorization() {
      this.lastfmAuthorizationEpoch += 1;
      this.lastfmAuthorizationCleanup?.();
      this.lastfmAuthorizationCleanup = null;
    },
    async sendProxyConfig() {
      if (this.proxyProtocol === 'noProxy') return;
      const config = this.settings.proxyConfig;
      if (
        config.server === '' ||
        !config.port ||
        config.protocol === 'noProxy'
      ) {
        this.showToast(locale.t('toast.proxyIncomplete'));
        return;
      }
      try {
        await sendDesktop('setProxy', config);
        this.showToast(locale.t('toast.proxyUpdated'));
        await relaunchDesktop();
      } catch (error) {
        console.error('[proxy] failed to update the native proxy', error);
        this.showToast(locale.t('toast.proxyUpdateFailed'));
      }
    },
    async disableProxy() {
      try {
        await sendDesktop('removeProxy');
        this.showToast(locale.t('toast.proxyDisabled'));
        await relaunchDesktop();
      } catch (error) {
        console.error('[proxy] failed to remove the native proxy', error);
        this.showToast(locale.t('toast.proxyDisableFailed'));
      }
    },
    clickOutside() {
      this.exitRecordShortcut();
    },
    formatShortcut(shortcut: string) {
      shortcut = shortcut
        .replaceAll('+', ' + ')
        .replace('Up', '↑')
        .replace('Down', '↓')
        .replace('Right', '→')
        .replace('Left', '←');
      const spaceKey =
        SPACE_KEY_LABELS[normalizePersistedLocale(this.settings.lang)];
      if (spaceKey) shortcut = shortcut.replace('Space', spaceKey);
      if (platformIsMac) {
        return shortcut
          .replace('CommandOrControl', '⌘')
          .replace('Command', '⌘')
          .replace('Alt', '⌥')
          .replace('Control', '⌃')
          .replace('Shift', '⇧');
      }
      return shortcut.replace('CommandOrControl', 'Ctrl');
    },
    readyToRecordShortcut(id: string, type: ShortcutKind) {
      if (type === 'globalShortcut' && this.enableGlobalShortcut === false) {
        return;
      }
      this.shortcutInput = { id, type, recording: true };
      this.recordedShortcut = [];
      (this.$refs['shortcutTable'] as HTMLElement).focus();
      void sendDesktop('switchGlobalShortcutStatusTemporary', 'disable');
    },
    handleShortcutKeydown(e: KeyboardEvent) {
      if (this.shortcutInput.recording === false) return;
      e.preventDefault();
      const recordedKey = { code: e.code, key: e.key };
      const identity = getRecordedShortcutKeyIdentity(recordedKey);
      if (
        this.recordedShortcut.some(
          key => getRecordedShortcutKeyIdentity(key) === identity
        )
      ) {
        return;
      }
      this.recordedShortcut.push(recordedKey);
      if (
        /^Key[A-Z]$/.test(e.code) ||
        /^Digit[0-9]$/.test(e.code) ||
        /^F(?:[1-9]|1[0-2])$/.test(e.code) ||
        e.code === 'Space' ||
        ['ArrowRight', 'ArrowLeft', 'ArrowUp', 'ArrowDown'].includes(e.key) || // Arrows
        validShortcutCodes.includes(e.key)
      ) {
        this.saveShortcut();
      }
    },
    handleShortcutKeyup(e: KeyboardEvent) {
      const identity = getRecordedShortcutKeyIdentity({
        code: e.code,
        key: e.key,
      });
      if (
        this.recordedShortcut.some(
          key => getRecordedShortcutKeyIdentity(key) === identity
        )
      ) {
        this.recordedShortcut = this.recordedShortcut.filter(
          key => getRecordedShortcutKeyIdentity(key) !== identity
        );
      }
    },
    saveShortcut() {
      const { id, type } = this.shortcutInput;
      if (type === '') return;
      const payload = {
        id,
        type,
        shortcut: this.recordedShortcutComputed,
      };
      this.updateShortcut(payload);
      void sendDesktop('updateShortcut', payload);
      void syncDesktopSettings(this.settings);
      this.showToast(locale.t('toast.shortcutsSaved'));
      this.recordedShortcut = [];
    },
    exitRecordShortcut() {
      if (this.shortcutInput.recording === false) return;
      this.shortcutInput = { id: '', type: '', recording: false };
      this.recordedShortcut = [];
      void sendDesktop('switchGlobalShortcutStatusTemporary', 'enable');
    },
    restoreDefaultShortcuts() {
      this.restoreDefaultShortcutsInStore();
      void sendDesktop('restoreDefaultShortcuts', this.settings);
    },
  },
});
</script>

<style lang="scss" scoped>
.settings-page {
  display: flex;
  justify-content: center;
  margin-top: 32px;
}
.container {
  margin-top: 24px;
  width: 720px;
}
h2 {
  margin-top: 48px;
  font-size: 36px;
  color: var(--color-text);
}

h3 {
  margin-top: 48px;
  padding-bottom: 12px;
  font-size: 26px;
  color: var(--color-text);
  border-bottom: 1px solid rgba(128, 128, 128, 0.18);
}

.user {
  display: flex;
  align-items: center;
  justify-content: space-between;
  background: var(--color-secondary-bg);
  color: var(--color-text);
  padding: 16px 20px;
  border-radius: 16px;
  margin-bottom: 48px;
  img.avatar {
    border-radius: 50%;
    height: 64px;
    width: 64px;
  }
  img.cvip {
    height: 13px;
    margin-right: 4px;
  }
  .left {
    display: flex;
    align-items: center;
    .info {
      margin-left: 24px;
    }
    .nickname {
      font-size: 20px;
      font-weight: 600;
      margin-bottom: 2px;
    }
    .extra-info {
      font-size: 13px;
      .text {
        opacity: 0.68;
      }
      .vip {
        display: flex;
        align-items: center;
      }
    }
  }
  .right {
    .svg-icon {
      height: 18px;
      width: 18px;
      margin-right: 4px;
    }
    button {
      display: flex;
      align-items: center;
      font-size: 18px;
      font-weight: 600;
      text-decoration: none;
      border-radius: 10px;
      padding: 8px 12px;
      opacity: 0.68;
      color: var(--color-text);
      transition: 0.2s;
      margin: {
        right: 12px;
        left: 12px;
      }
      &:hover {
        opacity: 1;
        background: #eaeffd;
        color: #335eea;
      }
      &:active {
        opacity: 1;
        transform: scale(0.92);
        transition: 0.2s;
      }
    }
  }
}

.item {
  margin: 24px 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  color: var(--color-text);

  .title {
    font-size: 16px;
    font-weight: 500;
    opacity: 0.78;
  }

  .description {
    font-size: 14px;
    margin-top: 0.5em;
    opacity: 0.7;
  }
}

.updater {
  align-items: flex-start;

  .left {
    max-width: 70%;
  }

  .updater-notes {
    white-space: pre-line;
  }

  button:disabled {
    cursor: default;
    opacity: 0.55;
    transform: none;
  }
}

select {
  min-width: 192px;
  max-width: 600px;
  font-weight: 600;
  border: none;
  padding: 8px 12px 8px 12px;
  border-radius: 8px;
  color: var(--color-text);
  background: var(--color-secondary-bg);
  appearance: none;
  &:focus {
    outline: none;
    color: var(--color-primary);
    background: var(--color-primary-bg);
  }
}

// Keep native affordances on WebView2 and WebKitGTK.
:global(body[data-platform='win32'] .settings-page select),
:global(body[data-platform='linux'] .settings-page select) {
  appearance: auto;
}

button {
  color: var(--color-text);
  background: var(--color-secondary-bg);
  padding: 8px 12px 8px 12px;
  font-weight: 600;
  border-radius: 8px;
  transition: 0.2s;
  &:hover {
    transform: scale(1.06);
  }
  &:active {
    transform: scale(0.94);
  }
}

input.text-input.margin-right-0 {
  margin-right: 0;
}
input.text-input {
  background: var(--color-secondary-bg);
  border: none;
  margin-right: 22px;
  padding: 8px 12px 8px 12px;
  border-radius: 8px;
  color: var(--color-text);
  font-weight: 600;
  font-size: 16px;
}
input::-webkit-outer-spin-button,
input::-webkit-inner-spin-button {
  -webkit-appearance: none;
}
input[type='number'] {
  -moz-appearance: textfield;
}

#proxy-form,
#real-ip {
  display: flex;
  align-items: center;
}
#proxy-form.disabled,
#real-ip.disabled {
  opacity: 0.47;
  button:hover {
    transform: unset;
  }
}

#shortcut-table {
  font-size: 14px;
  /* border: 1px solid black; */
  user-select: none;
  color: var(--color-text);
  .row {
    display: flex;
  }
  .row.row-head {
    opacity: 0.58;
    font-size: 13px;
    font-weight: 500;
  }
  .col {
    min-width: 192px;
    padding: 8px;
    display: flex;
    align-items: center;
    /* border: 1px solid red; */
    &:first-of-type {
      padding-left: 0;
      min-width: 128px;
    }
  }
  .keyboard-input {
    font-weight: 600;
    background-color: var(--color-secondary-bg);
    padding: 8px 12px 8px 12px;
    border-radius: 0.5rem;
    min-width: 146px;
    min-height: 34px;
    box-sizing: border-box;
    &.active {
      color: var(--color-primary);
      background-color: var(--color-primary-bg);
    }
  }
  .restore-default-shortcut {
    margin-top: 12px;
  }
  &.global-disabled {
    .row .col:last-child {
      opacity: 0.48;
    }
    .row.row-head .col:last-child {
      opacity: 1;
    }
  }
  &:focus {
    outline: none;
  }
}

.footer {
  text-align: center;
  margin-top: 6rem;
  color: var(--color-text);
  font-weight: 600;
  .author {
    font-size: 0.9rem;
  }
  .version {
    font-size: 0.88rem;
    opacity: 0.58;
    margin-top: -10px;
  }
}

.beforeAnimation {
  -webkit-transition: 0.2s cubic-bezier(0.24, 0, 0.5, 1);
  transition: 0.2s cubic-bezier(0.24, 0, 0.5, 1);
}
.afterAnimation {
  box-shadow: 0 0 0 1px hsla(0, 0%, 0%, 0.1), 0 4px 0px 0 hsla(0, 0%, 0%, 0.04),
    0 4px 9px hsla(0, 0%, 0%, 0.13), 0 3px 3px hsla(0, 0%, 0%, 0.05);
  -webkit-transition: 0.35s cubic-bezier(0.54, 1.6, 0.5, 1);
  transition: 0.35s cubic-bezier(0.54, 1.6, 0.5, 1);
}
.toggle {
  margin: auto;
}
.toggle input {
  opacity: 0;
  position: absolute;
}
.toggle input + label {
  position: relative;
  display: inline-block;
  -webkit-user-select: none;
  -moz-user-select: none;
  -ms-user-select: none;
  user-select: none;
  -webkit-transition: 0.4s ease;
  transition: 0.4s ease;
  height: 32px;
  width: 52px;
  background: var(--color-secondary-bg);
  border-radius: 8px;
}
.toggle input + label:before {
  content: '';
  position: absolute;
  display: block;
  -webkit-transition: 0.2s cubic-bezier(0.24, 0, 0.5, 1);
  transition: 0.2s cubic-bezier(0.24, 0, 0.5, 1);
  height: 32px;
  width: 52px;
  top: 0;
  left: 0;
  border-radius: 8px;
}
.toggle input + label:after {
  content: '';
  position: absolute;
  display: block;
  box-shadow: 0 0 0 1px hsla(0, 0%, 0%, 0.02), 0 4px 0px 0 hsla(0, 0%, 0%, 0.01),
    0 4px 9px hsla(0, 0%, 0%, 0.08), 0 3px 3px hsla(0, 0%, 0%, 0.03);
  -webkit-transition: 0.35s cubic-bezier(0.54, 1.6, 0.5, 1);
  transition: 0.35s cubic-bezier(0.54, 1.6, 0.5, 1);
  background: #fff;
  height: 20px;
  width: 20px;
  top: 6px;
  left: 6px;
  border-radius: 6px;
}
.toggle input:checked + label:before {
  background: var(--color-primary-gradient);
  -webkit-transition: width 0.2s cubic-bezier(0, 0, 0, 0.1);
  transition: width 0.2s cubic-bezier(0, 0, 0, 0.1);
}
.toggle input:checked + label:after {
  left: 26px;
}
</style>
