export interface ApiForwardManifest {
  allowedPaths: string[];
  crypto: string;
}

export interface ExtractedNcmRoute {
  id: string;
  method: 'GET' | 'POST';
  path: string;
  requestBuilder: string;
  nodeAdapter: string;
  decoder: string;
  apiForward?: ApiForwardManifest;
}

export interface SidecarRouteManifestEntry extends ExtractedNcmRoute {
  rustAdapter: string;
  comparator: string[];
  idempotent: boolean;
}

export interface RouteManifestOptions {
  rootDir?: string;
}

export function extractProductionNcmRoutes(
  options?: RouteManifestOptions
): ExtractedNcmRoute[];

export function loadSidecarRouteManifest(
  options?: RouteManifestOptions
): unknown;

export function validateSidecarRouteManifest(
  manifest: unknown
): SidecarRouteManifestEntry[];

export function comparableManifestRoute(
  route: SidecarRouteManifestEntry
): ExtractedNcmRoute;

export function verifySidecarRouteManifest(options?: RouteManifestOptions): {
  manifest: SidecarRouteManifestEntry[];
  actual: ExtractedNcmRoute[];
};
