import request from '@/utils/request';
import type {
  Album,
  Artist,
  MusicVideo,
  Playlist,
  Track,
  UserProfile,
} from '@/types/domain';
import type { ApiResponse } from './types';
import {
  decodeAlbum,
  decodeApiResponse,
  decodeArray,
  decodeArtist,
  decodeMusicVideo,
  decodeNumber,
  decodeOptionalArray,
  decodeOptionalNumber,
  decodeOptionalString,
  decodePlaylist,
  decodeRecord,
  decodeTrack,
  decodeUserProfile,
} from './decoders';
import type { Decoder, ValueDecoder } from './decoders';

interface PlayHistoryItem {
  song: Track;
  playCount: number;
}

const decodeCloudTrack: ValueDecoder<Track> = (input, context, field) => {
  const track = decodeRecord(input, context, field);
  const songId = decodeOptionalNumber(
    track['songId'],
    context,
    `${field}.songId`
  );
  const id =
    track['id'] === undefined
      ? decodeNumber(songId, context, `${field}.songId`)
      : decodeNumber(track['id'], context, `${field}.id`);
  return {
    ...track,
    id,
    ...(songId === undefined ? {} : { songId }),
  };
};

const decodeAccountResponse: Decoder<
  ApiResponse & { code: number; profile: UserProfile }
> = (input, context) => {
  const response = decodeRecord(input, context);
  return {
    ...response,
    code: decodeNumber(response['code'], context, '$.code'),
    profile: decodeUserProfile(response['profile'], context, '$.profile'),
  };
};

const decodeOptionalPlaylistsResponse: Decoder<
  ApiResponse & { playlist?: Playlist[] }
> = (input, context) => {
  const response = decodeRecord(input, context);
  const playlist = decodeOptionalArray(
    response['playlist'],
    context,
    '$.playlist',
    decodePlaylist
  );
  return { ...response, ...(playlist === undefined ? {} : { playlist }) };
};

const decodePlayHistoryItem: ValueDecoder<PlayHistoryItem> = (
  input,
  context,
  field
) => {
  const item = decodeRecord(input, context, field);
  return {
    song: decodeTrack(item['song'], context, `${field}.song`),
    playCount: decodeNumber(item['playCount'], context, `${field}.playCount`),
  };
};

const decodePlayHistoryResponse: Decoder<
  ApiResponse & { allData?: PlayHistoryItem[]; weekData?: PlayHistoryItem[] }
> = (input, context) => {
  const response = decodeRecord(input, context);
  const allData = decodeOptionalArray(
    response['allData'],
    context,
    '$.allData',
    decodePlayHistoryItem
  );
  const weekData = decodeOptionalArray(
    response['weekData'],
    context,
    '$.weekData',
    decodePlayHistoryItem
  );
  return {
    ...response,
    ...(allData === undefined ? {} : { allData }),
    ...(weekData === undefined ? {} : { weekData }),
  };
};

const decodeLikedSongIdsResponse: Decoder<ApiResponse & { ids?: number[] }> = (
  input,
  context
) => {
  const response = decodeRecord(input, context);
  const ids = decodeOptionalArray(
    response['ids'],
    context,
    '$.ids',
    decodeNumber
  );
  return { ...response, ...(ids === undefined ? {} : { ids }) };
};

const decodeOptionalAlbumsResponse: Decoder<
  ApiResponse & { data?: Album[] }
> = (input, context) => {
  const response = decodeRecord(input, context);
  const data = decodeOptionalArray(
    response['data'],
    context,
    '$.data',
    decodeAlbum
  );
  return { ...response, ...(data === undefined ? {} : { data }) };
};

const decodeOptionalArtistsResponse: Decoder<
  ApiResponse & { data?: Artist[] }
> = (input, context) => {
  const response = decodeRecord(input, context);
  const data = decodeOptionalArray(
    response['data'],
    context,
    '$.data',
    decodeArtist
  );
  return { ...response, ...(data === undefined ? {} : { data }) };
};

const decodeOptionalMvsResponse: Decoder<
  ApiResponse & { data?: MusicVideo[] }
> = (input, context) => {
  const response = decodeRecord(input, context);
  const data = decodeOptionalArray(
    response['data'],
    context,
    '$.data',
    decodeMusicVideo
  );
  return { ...response, ...(data === undefined ? {} : { data }) };
};

const decodeUploadResponse: Decoder<
  ApiResponse & { code: number; privateCloud: Track }
> = (input, context) => {
  const response = decodeRecord(input, context);
  return {
    ...response,
    code: decodeNumber(response['code'], context, '$.code'),
    privateCloud: decodeCloudTrack(
      response['privateCloud'],
      context,
      '$.privateCloud'
    ),
  };
};

const decodeOptionalTracksResponse: Decoder<
  ApiResponse & { data?: Track[] }
> = (input, context) => {
  const response = decodeRecord(input, context);
  const data = decodeOptionalArray(
    response['data'],
    context,
    '$.data',
    decodeCloudTrack
  );
  return { ...response, ...(data === undefined ? {} : { data }) };
};

const decodeCloudDeleteResponse: Decoder<
  ApiResponse & { code: number; message?: string }
> = (input, context) => {
  const response = decodeRecord(input, context);
  const message = decodeOptionalString(
    response['message'],
    context,
    '$.message'
  );
  return {
    ...response,
    code: decodeNumber(response['code'], context, '$.code'),
    ...(message === undefined ? {} : { message }),
  };
};

export function userDetail(uid: number): Promise<ApiResponse> {
  return request<ApiResponse>(
    {
      url: '/user/detail',
      method: 'get',
      params: {
        uid,
        timestamp: new Date().getTime(),
      },
    },
    decodeApiResponse
  );
}

export function userAccount() {
  return request<ApiResponse & { code: number; profile: UserProfile }>(
    {
      url: '/user/account',
      method: 'get',
      params: {
        timestamp: new Date().getTime(),
      },
    },
    decodeAccountResponse
  );
}

export function userPlaylist(params: {
  uid: number;
  limit: number;
  offset?: number;
  timestamp?: number;
}) {
  return request<ApiResponse & { playlist?: Playlist[] }>(
    {
      url: '/user/playlist',
      method: 'get',
      params,
    },
    decodeOptionalPlaylistsResponse
  );
}

export function userPlayHistory(params: { uid: number; type: number }) {
  return request<
    ApiResponse & { allData?: PlayHistoryItem[]; weekData?: PlayHistoryItem[] }
  >(
    {
      url: '/user/record',
      method: 'get',
      params,
    },
    decodePlayHistoryResponse
  );
}

export function userLikedSongsIDs(uid: number) {
  return request<ApiResponse & { ids?: number[] }>(
    {
      url: '/likelist',
      method: 'get',
      params: {
        uid,
        timestamp: new Date().getTime(),
      },
    },
    decodeLikedSongIdsResponse
  );
}

export function dailySignin(type = 0): Promise<ApiResponse> {
  return request<ApiResponse>(
    {
      url: '/daily_signin',
      method: 'post',
      params: {
        type,
        timestamp: new Date().getTime(),
      },
    },
    decodeApiResponse
  );
}

export function likedAlbums(params: { limit: number; offset?: number }) {
  return request<ApiResponse & { data?: Album[] }>(
    {
      url: '/album/sublist',
      method: 'get',
      params: {
        limit: params.limit,
        timestamp: new Date().getTime(),
      },
    },
    decodeOptionalAlbumsResponse
  );
}

export function likedArtists(params: { limit: number; offset?: number }) {
  return request<ApiResponse & { data?: Artist[] }>(
    {
      url: '/artist/sublist',
      method: 'get',
      params: {
        limit: params.limit,
        timestamp: new Date().getTime(),
      },
    },
    decodeOptionalArtistsResponse
  );
}

export function likedMVs(params: { limit: number; offset?: number }) {
  return request<ApiResponse & { data?: MusicVideo[] }>(
    {
      url: '/mv/sublist',
      method: 'get',
      params: {
        limit: params.limit,
        timestamp: new Date().getTime(),
      },
    },
    decodeOptionalMvsResponse
  );
}

export function uploadSong(
  file: Blob
): Promise<ApiResponse & { code: number; privateCloud: Track }> {
  const formData = new FormData();
  formData.append('songFile', file);
  return request<ApiResponse & { code: number; privateCloud: Track }>(
    {
      url: '/cloud',
      method: 'post',
      params: {
        timestamp: new Date().getTime(),
      },
      data: formData,
      headers: {
        'Content-Type': 'multipart/form-data',
      },
      timeout: 400000,
    },
    decodeUploadResponse
  );
}

export function cloudDisk(
  params: { limit?: number; offset?: number; timestamp?: number } = {}
) {
  params.timestamp = new Date().getTime();
  return request<ApiResponse & { data?: Track[] }>(
    {
      url: '/user/cloud',
      method: 'get',
      params,
    },
    decodeOptionalTracksResponse
  );
}

export function cloudDiskTrackDetail(id: number): Promise<ApiResponse> {
  return request<ApiResponse>(
    {
      url: '/user/cloud/detail',
      method: 'get',
      params: {
        timestamp: new Date().getTime(),
        id,
      },
    },
    decodeApiResponse
  );
}

export function cloudDiskTrackDelete(
  id: number | number[]
): Promise<ApiResponse & { code: number; message?: string }> {
  return request<ApiResponse & { code: number; message?: string }>(
    {
      url: '/user/cloud/del',
      method: 'get',
      params: {
        timestamp: new Date().getTime(),
        id,
      },
    },
    decodeCloudDeleteResponse
  );
}
