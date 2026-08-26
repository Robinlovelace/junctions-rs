#' Detect road junctions with the Rust core
#'
#' @param roads A list of roads. Each item must have `id`, `coordinates` (a
#'   matrix/list of projected two-number coordinate positions) and optional `level`.
#' @param buffer_m Polygon half-width in projected metres.
#' @param min_arms Minimum number of contributing roads.
#' @param cluster_distance_m Candidate-node merge tolerance in metres.
#' @return A data frame with a list-column `polygon` of closed coordinate rings.
#' @export
junctions <- function(roads, buffer_m = 5, min_arms = 3, cluster_distance_m = 0.01) {
  payload <- jsonlite::toJSON(roads, auto_unbox = TRUE, digits = NA)
  parsed <- jsonlite::fromJSON(
    junctions_json(payload, buffer_m, as.integer(min_arms), cluster_distance_m),
    simplifyDataFrame = TRUE
  )
  if (is.null(parsed) || length(parsed) == 0) return(data.frame())
  parsed
}
