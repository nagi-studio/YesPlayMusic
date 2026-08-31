import http from 'node:http';
import https from 'node:https';
import net from 'node:net';
import tls from 'node:tls';
import type {
  IncomingMessage,
  RequestOptions,
  ServerResponse,
} from 'node:http';
import type { Socket } from 'node:net';
import type { Duplex } from 'node:stream';

const LOOPBACK_HOST = '127.0.0.1';

export interface ProxyRelay {
  server: http.Server;
  close(): Promise<void>;
}

export interface ProxyRelayOptions {
  port: number;
  upstreamProxy: string;
  upstreamTlsCa?: string;
}

interface Target {
  host: string;
  port: number;
}

function unbracketHost(host: string): string {
  return host.startsWith('[') && host.endsWith(']') ? host.slice(1, -1) : host;
}

function appendBuffer(current: Buffer, chunk: Buffer): Buffer {
  const combined = Buffer.allocUnsafe(current.length + chunk.length);
  for (let index = 0; index < current.length; index += 1) {
    combined[index] = current[index] ?? 0;
  }
  for (let index = 0; index < chunk.length; index += 1) {
    combined[current.length + index] = chunk[index] ?? 0;
  }
  return combined;
}

export function isLoopbackProxyHost(host: string): boolean {
  const normalized = unbracketHost(host).toLowerCase().replace(/\.$/, '');
  return (
    normalized === '127.0.0.1' ||
    normalized === 'localhost' ||
    normalized === '::1'
  );
}

function assertPort(port: number): void {
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error('proxy relay port must be between 1 and 65535');
  }
}

export function parseUpstreamProxy(value: string): URL {
  if (value.trim() !== value) {
    throw new Error('upstream proxy must be a valid HTTP(S) URL');
  }
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error('upstream proxy must be a valid HTTP(S) URL');
  }
  if (
    !['http:', 'https:'].includes(url.protocol) ||
    !url.hostname ||
    url.username ||
    url.password ||
    url.pathname !== '/' ||
    value.includes('?') ||
    value.includes('#') ||
    url.search ||
    url.hash ||
    url.port === '0'
  ) {
    throw new Error(
      'upstream proxy must contain only an HTTP(S) host and port'
    );
  }
  return url;
}

function proxyPort(upstream: URL): number {
  return upstream.port
    ? Number(upstream.port)
    : upstream.protocol === 'https:'
    ? 443
    : 80;
}

function failResponse(
  response: ServerResponse,
  status: number,
  message: string
): void {
  if (!response.headersSent) {
    response.writeHead(status, { 'content-type': 'text/plain; charset=utf-8' });
  }
  response.end(message);
}

function failTunnel(socket: Duplex, status: number, message: string): void {
  if (socket.destroyed) return;
  socket.end(
    `HTTP/1.1 ${status} ${message}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`
  );
}

function requestHeaders(request: IncomingMessage): http.OutgoingHttpHeaders {
  const headers = { ...request.headers };
  delete headers['proxy-authorization'];
  delete headers['proxy-connection'];
  return headers;
}

function parseHttpTarget(request: IncomingMessage): {
  url: URL;
  upstreamPath: string;
} {
  const requestTarget = request.url ?? '';
  if (/^http:\/\//i.test(requestTarget)) {
    const url = new URL(requestTarget);
    if (
      url.protocol !== 'http:' ||
      url.username ||
      url.password ||
      url.hash ||
      url.port === '0'
    ) {
      throw new Error('invalid HTTP proxy target');
    }
    return { url, upstreamPath: requestTarget };
  }
  if (!requestTarget.startsWith('/')) {
    throw new Error('HTTP proxy target must use absolute-form or origin-form');
  }
  const host = request.headers.host;
  if (!host) throw new Error('HTTP proxy target is missing Host');
  const url = new URL(requestTarget, `http://${host}`);
  if (url.port === '0') throw new Error('invalid HTTP proxy target');
  return { url, upstreamPath: url.href };
}

function parseConnectTarget(authority: string): Target {
  if (!authority || /[/?#]/.test(authority)) {
    throw new Error('invalid CONNECT authority');
  }
  let url: URL;
  try {
    url = new URL(`http://${authority}`);
  } catch {
    throw new Error('invalid CONNECT authority');
  }
  if (
    url.username ||
    url.password ||
    url.pathname !== '/' ||
    !url.hostname ||
    url.port === '0'
  ) {
    throw new Error('invalid CONNECT authority');
  }
  return {
    host: unbracketHost(url.hostname),
    port: url.port ? Number(url.port) : 443,
  };
}

function pipeHttpRequest(
  request: IncomingMessage,
  response: ServerResponse,
  options: RequestOptions,
  secure = false
): void {
  const transport = secure ? https : http;
  const outgoing = transport.request(options, upstreamResponse => {
    response.writeHead(
      upstreamResponse.statusCode ?? 502,
      upstreamResponse.statusMessage,
      upstreamResponse.headers
    );
    upstreamResponse.once('error', error => response.destroy(error));
    upstreamResponse.pipe(response);
  });
  outgoing.once('error', error => {
    if (response.headersSent) response.destroy(error);
    else failResponse(response, 502, `Proxy request failed: ${error.message}`);
  });
  request.once('aborted', () => outgoing.destroy());
  request.pipe(outgoing);
}

function forwardHttpRequest(
  request: IncomingMessage,
  response: ServerResponse,
  upstream: URL,
  upstreamTlsCa?: string
): void {
  let target: ReturnType<typeof parseHttpTarget>;
  try {
    target = parseHttpTarget(request);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    failResponse(response, 400, message);
    return;
  }

  const headers = requestHeaders(request);
  if (isLoopbackProxyHost(target.url.hostname)) {
    pipeHttpRequest(request, response, {
      hostname: unbracketHost(target.url.hostname),
      port: target.url.port ? Number(target.url.port) : 80,
      method: request.method,
      path: `${target.url.pathname}${target.url.search}`,
      headers,
    });
    return;
  }

  const upstreamHost = unbracketHost(upstream.hostname);
  pipeHttpRequest(
    request,
    response,
    {
      hostname: upstreamHost,
      port: proxyPort(upstream),
      method: request.method,
      path: target.upstreamPath,
      headers,
      ...(net.isIP(upstreamHost) === 0 ? { servername: upstreamHost } : {}),
      ...(upstreamTlsCa === undefined ? {} : { ca: upstreamTlsCa }),
    },
    upstream.protocol === 'https:'
  );
}

function connectDirectly(
  client: Duplex,
  head: Buffer,
  target: Target,
  trackSocket: (socket: Socket) => void
): void {
  const targetSocket = net.connect(target.port, target.host);
  trackSocket(targetSocket);
  let tunnelReady = false;
  targetSocket.once('connect', () => {
    tunnelReady = true;
    client.write('HTTP/1.1 200 Connection Established\r\n\r\n');
    if (head.length) targetSocket.write(Uint8Array.from(head));
    client.pipe(targetSocket).pipe(client);
  });
  targetSocket.once('error', () => {
    if (tunnelReady) client.destroy();
    else failTunnel(client, 502, 'Bad Gateway');
  });
  client.once('error', () => targetSocket.destroy());
  client.once('close', () => targetSocket.destroy());
}

function connectThroughUpstream(
  client: Duplex,
  head: Buffer,
  authority: string,
  upstream: URL,
  trackSocket: (socket: Socket) => void,
  upstreamTlsCa?: string
): void {
  const upstreamHost = unbracketHost(upstream.hostname);
  const secure = upstream.protocol === 'https:';
  const upstreamSocket = secure
    ? tls.connect({
        host: upstreamHost,
        port: proxyPort(upstream),
        ALPNProtocols: ['http/1.1'],
        ...(net.isIP(upstreamHost) === 0 ? { servername: upstreamHost } : {}),
        ...(upstreamTlsCa === undefined ? {} : { ca: upstreamTlsCa }),
      })
    : net.connect(proxyPort(upstream), upstreamHost);
  trackSocket(upstreamSocket);
  let responseBuffer = Buffer.alloc(0);
  let responseStarted = false;
  let tunnelReady = false;
  upstreamSocket.once(secure ? 'secureConnect' : 'connect', () => {
    upstreamSocket.write(
      `CONNECT ${authority} HTTP/1.1\r\nHost: ${authority}\r\nProxy-Connection: keep-alive\r\n\r\n`
    );
  });
  const onData = (chunk: Buffer): void => {
    responseBuffer = appendBuffer(responseBuffer, chunk);
    if (responseBuffer.length > 64 * 1024) {
      upstreamSocket.destroy();
      failTunnel(client, 502, 'Bad Gateway');
      return;
    }
    const boundary = responseBuffer.indexOf('\r\n\r\n');
    if (boundary < 0) return;
    upstreamSocket.off('data', onData);
    const headerEnd = boundary + 4;
    const header = responseBuffer.subarray(0, headerEnd);
    const status = /^HTTP\/\d\.\d (\d{3})(?: |\r)/.exec(
      header.toString('latin1')
    );
    responseStarted = true;
    client.write(Uint8Array.from(header));
    if (status?.[1] !== '200') {
      upstreamSocket.destroy();
      client.end();
      return;
    }
    tunnelReady = true;
    const upstreamHead = responseBuffer.subarray(headerEnd);
    if (upstreamHead.length) client.write(Uint8Array.from(upstreamHead));
    if (head.length) upstreamSocket.write(Uint8Array.from(head));
    client.pipe(upstreamSocket).pipe(client);
  };
  upstreamSocket.on('data', onData);
  upstreamSocket.once('error', () => {
    if (tunnelReady) client.destroy();
    else if (!responseStarted) failTunnel(client, 502, 'Bad Gateway');
  });
  upstreamSocket.once('end', () => {
    if (!responseStarted) failTunnel(client, 502, 'Bad Gateway');
  });
  client.once('close', () => upstreamSocket.destroy());
}

function forwardConnect(
  request: IncomingMessage,
  client: Duplex,
  head: Buffer,
  upstream: URL,
  trackSocket: (socket: Socket) => void,
  upstreamTlsCa?: string
): void {
  const authority = request.url ?? '';
  let target: Target;
  try {
    target = parseConnectTarget(authority);
  } catch {
    failTunnel(client, 400, 'Bad Request');
    return;
  }
  if (isLoopbackProxyHost(target.host)) {
    connectDirectly(client, head, target, trackSocket);
    return;
  }
  connectThroughUpstream(
    client,
    head,
    authority,
    upstream,
    trackSocket,
    upstreamTlsCa
  );
}

export async function startWebviewProxyRelay(
  options: ProxyRelayOptions
): Promise<ProxyRelay> {
  assertPort(options.port);
  const upstream = parseUpstreamProxy(options.upstreamProxy);
  const sockets = new Set<Socket>();
  const trackSocket = (socket: Socket): void => {
    sockets.add(socket);
    socket.once('close', () => sockets.delete(socket));
  };
  const server = http.createServer((request, response) => {
    forwardHttpRequest(request, response, upstream, options.upstreamTlsCa);
  });
  server.on('connect', (request, socket, head) => {
    forwardConnect(
      request,
      socket,
      head,
      upstream,
      trackSocket,
      options.upstreamTlsCa
    );
  });
  server.on('connection', trackSocket);

  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(options.port, LOOPBACK_HOST, () => {
      server.off('error', reject);
      resolve();
    });
  });

  let closing: Promise<void> | null = null;
  return {
    server,
    close() {
      if (closing) return closing;
      closing = new Promise<void>((resolve, reject) => {
        server.close(error => (error ? reject(error) : resolve()));
        for (const socket of sockets) socket.destroy();
      });
      return closing;
    },
  };
}
