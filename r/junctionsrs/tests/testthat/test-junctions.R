test_that("R wrapper reaches the shared Rust core", {
  roads <- list(
    list(id = "west", coordinates = list(c(-10, 0), c(0, 0)), node_ids = c("west", "centre"), level = 0L),
    list(id = "east", coordinates = list(c(0, 0), c(10, 0)), node_ids = c("centre", "east"), level = 0L),
    list(id = "north", coordinates = list(c(0, 0), c(0, 10)), node_ids = c("centre", "north"), level = 0L)
  )
  result <- junctions(roads)
  expect_equal(nrow(result), 1L)
  expect_equal(result$num_arms[[1]], 3)
  expect_equal(result$node_ids[[1]], "centre")
  expect_equal(result$way_ids[[1]], c("east", "north", "west"))
})

test_that("R wrapper preserves singleton node IDs as arrays", {
  roads <- list(
    list(id = "east", coordinates = list(c(0, 0), c(10, 0)), node_ids = "centre", level = 0L),
    list(id = "north", coordinates = list(c(0, 0), c(0, 10)), node_ids = "centre", level = 0L),
    list(id = "west", coordinates = list(c(0, 0), c(-10, 0)), node_ids = "centre", level = 0L)
  )
  result <- junctions(roads)
  expect_equal(nrow(result), 1L)
  expect_equal(result$node_ids[[1]], "centre")
})
